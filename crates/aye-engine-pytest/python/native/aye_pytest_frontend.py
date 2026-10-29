"""aye's pytest front-end: it reads a pytest suite into aye's model.

The worker installs this module as ``aye_pytest_frontend`` when the pytest
adapter runs (on aye's engine, always), after ``aye_model`` and
aye's own ``pytest`` module (installed as ``pytest``). It does what pytest's
collection does, natively, and never runs a test (the engine does):

- Configure: the settings (the recognizer's ``native`` context when it has
  one, else the configuration file the context names, read here), the
  options (``addopts``, ``PYTEST_ADDOPTS``, then the arguments after ``--``,
  parsed against pytest's builtin options and the conftests'), the
  ``pythonpath`` entries, the start-up conftests (those of the user's paths
  and their ``test*`` directories), their ``pytest_addoption``,
  ``pytest_configure`` and ``pytest_sessionstart``.
- pytest's logging plugin's handlers stay on the root logger around
  ``pytest_sessionstart``, collection, the test loop (the glue's) and
  ``pytest_sessionfinish`` (``Frontend.logging_plugin``), so a
  ``logging.basicConfig()`` a test module or a test calls does nothing, as
  under pytest.
- List: the walk from the ``confcutdir`` down to each file, a directory's
  conftests loading as it is collected (an import error fails the
  directory, a skip skips it), with pytest's ignore rules
  (``collect_ignore``, ``collect_ignore_glob``, ``--ignore``,
  ``--ignore-glob``, ``norecursedirs``, virtualenvs, ``pytest_ignore_collect``);
  each module imported as pytest imports it (``prepend``, ``append`` or
  ``importlib``) under pytest's collection-time warning filters; members in
  pytest's order (definition order, inherited tests first, nested classes,
  ``__test__``, classes with ``__init__`` left out with pytest's warning,
  unittest ``TestCase`` classes whatever their name with their methods by
  name); fixtures registered with pytest's visibility and override chains;
  xunit ``setup_*``/``teardown_*`` and unittest's ``setUpClass`` as auto
  resources; ``pytest_generate_tests`` in pluggy's order (the class's, the
  module's, conftests', fixture ``params``, then ``parametrize`` marks) and
  pytest's ids; the ``pytest_collection_modifyitems`` chain, with pytest's
  ``reorder_items`` where pytest runs it.
- Listing is cheap where pytest's semantics allow: aye's own loops (the
  parametrized items, the cases, the refinement's wire format) run without
  automatic garbage collection (``no_automatic_gc``), never around the
  suite's code; ``settle_gc`` then collects the younger generations once
  (what listing made, never the rest of the heap); what most nodes never use
  (``stash``, ``extra_keyword_matches``, finalizers, ``user_properties``,
  ``funcargs``) is made when first asked for; the names ``-k`` matches are
  read once per class or module and function (``case_dicts``).
- A session is listed in steps (``extend``: a worker's refinement, then
  any file a batch needs that wasn't listed): each file once, the
  ``pytest_collection_modifyitems`` hooks over each step's new items, the
  session's ``items`` growing; what a file's collection reported is kept by
  file (``file_diagnostics``). A configuration that failed fails every
  step. ``collect`` is one step on its own, narrowed by a batch's
  selection before the hooks.
- The model: one ``aye_model.Case`` per pytest item, in pytest's order; its
  ``Node`` chain (packages, the module, classes), each with every resource
  visible from it, closest definition first; the ``Policy`` (xfail
  strictness, the condition namespace, pytest's outcome exceptions) and the
  ``Hooks`` (the conftests' ``pytest_runtest_setup``, ``_call`` and
  ``_teardown``, which see an item object: ``case.extra["item"]``). A mark
  added to a node calls ``marks_changed`` (the glue's: the engine's
  ``tags_changed``), so the engine reads the case's marks again.
- A refined case's ``affinity`` names only the costly lifetimes it shares
  (``Frontend.affinity``): ``module:<path>`` when a module- or
  package-scoped resource in its closure runs the user's code (fixtures,
  ``setup_module``, package setups), then ``class:<id>`` when a class-scoped
  one does (fixtures, ``setup_class``, a ``TestCase``'s own
  ``setUpClass``/``tearDownClass``/class cleanups); a file the
  import-mismatch rule groups (its module name, or a helper it imports, in
  the context's ``colliding_modules``) keeps ``import:NAME`` first. Other
  cases are free: the runner may split their files across workers.

Ids are aye's: the file relative to aye's root, then pytest's names.
pytest's own items (``Function``, ``Class``, ``Module``…), ``Config``,
``Metafunc`` and ``Parser`` are this module's classes, subclasses of aye's
``pytest`` module's types, so conftest hooks get what they expect.

What pytest would do and this front-end doesn't is a diagnostic that says
so, never a different result: a conftest hook it doesn't run, a hook
wrapper, a third-party plugin in ``pytest_plugins`` or ``-p`` make a
``usage-error`` and the tests below that conftest aren't collected; an
option it doesn't know is a warning.

It imports only the standard library, ``aye_model`` and aye's ``pytest``
(never pytest's ``_pytest``), and runs on Python 3.10 and later.
"""

from __future__ import annotations

import argparse
import ast
import builtins
import configparser
import enum
import fnmatch
import functools
import gc
import glob
import importlib
import importlib.machinery
import importlib.util
import inspect
import itertools
import logging
import math
import os
import platform
import re
import shlex
import sys
import tempfile
import traceback
import types
import unittest
import warnings
from bisect import bisect_left
from collections import Counter, OrderedDict, defaultdict, deque
from collections.abc import Callable, Collection, Iterable, Iterator, MutableMapping, Sequence
from contextlib import ExitStack, contextmanager, nullcontext
from pathlib import Path
from typing import Any, ClassVar, NamedTuple

import aye_model
import pytest
from aye_model import Case, Hooks, Policy, ResourceDef, Scope, Tag

try:
    import tomllib
except ImportError:  # Python 3.10: TOML configuration comes from the context
    tomllib = None

# pytest's absent value (``NOTSET``): the shim's when it has one.
NOTSET: Any = getattr(pytest, "NOTSET", object())
HIDDEN_PARAM: Any = getattr(pytest, "HIDDEN_PARAM", object())

# The exceptions pytest's outcome helpers raise.
SKIPPED: type[BaseException] = pytest.skip.Exception
FAILED: type[BaseException] = pytest.fail.Exception
XFAILED: type[BaseException] = pytest.xfail.Exception
EXIT: type[BaseException] = pytest.exit.Exception
# What pytest catches in a phase or while collecting (``TEST_OUTCOME``).
OUTCOMES: tuple[type[BaseException], ...] = (SKIPPED, FAILED, Exception)

COLLECT_ERROR: type[Exception] = getattr(
    getattr(pytest, "Collector", None), "CollectError", type("CollectError", (Exception,), {})
)

# Scopes above function, outermost first (``reorder_items``).
HIGH_SCOPES = (Scope.SESSION, Scope.PACKAGE, Scope.MODULE, Scope.CLASS)


class _Empty:
    pass


# Names pytest never asks its item hooks about: the dunders of an empty
# module, class and instance.
IGNORED_ATTRIBUTES = frozenset(
    {
        *dir(types.ModuleType("empty_module")),
        "__builtins__",
        "__file__",
        "__cached__",
        *dir(_Empty),
        *dir(_Empty()),
    }
)

# The conftest hooks this front-end runs: name -> the arguments pytest passes.
HOOKS: dict[str, tuple[str, ...]] = {
    "pytest_addoption": ("parser", "pluginmanager"),
    "pytest_configure": ("config",),
    "pytest_unconfigure": ("config",),
    "pytest_sessionstart": ("session",),
    "pytest_sessionfinish": ("session", "exitstatus"),
    "pytest_collection": ("session",),
    "pytest_collection_modifyitems": ("session", "config", "items"),
    "pytest_collection_finish": ("session",),
    "pytest_deselected": ("items",),
    "pytest_itemcollected": ("item",),
    "pytest_report_header": ("config", "start_path", "startdir"),
    "pytest_ignore_collect": ("collection_path", "path", "config"),
    "pytest_make_parametrize_id": ("config", "val", "argname"),
    "pytest_markeval_namespace": ("config",),
    "pytest_generate_tests": ("metafunc",),
    "pytest_runtest_setup": ("item",),
    "pytest_runtest_call": ("item",),
    "pytest_runtest_teardown": ("item", "nextitem"),
}
# Hooks that only display (they never change which tests exist or their
# outcomes): accepted, never called.
DISPLAY_HOOKS = frozenset(
    {"pytest_terminal_summary", "pytest_report_collectionfinish", "pytest_report_teststatus"}
)
FIRSTRESULT = frozenset(
    {"pytest_ignore_collect", "pytest_make_parametrize_id", "pytest_collection"}
)

# Builtin plugins ``-p no:NAME`` turns off, with the fixtures they provide.
PLUGIN_FIXTURES: dict[str, tuple[str, ...]] = {
    "tmpdir": ("tmp_path", "tmp_path_factory", "tmpdir", "tmpdir_factory"),
    "legacypath": ("tmpdir", "tmpdir_factory"),
    "logging": ("caplog",),
    "capture": ("capsys", "capfd", "capsysbinary", "capfdbinary", "capteesys"),
    "monkeypatch": ("monkeypatch",),
    "recwarn": ("recwarn",),
    "cacheprovider": ("cache",),
    "subtests": ("subtests",),
    "junitxml": ("record_property", "record_xml_attribute", "record_testsuite_property"),
}
ESSENTIAL_PLUGINS = frozenset({"mark", "main", "runner", "fixtures", "helpconfig"})
# Installed plugins (``pytest11`` entry points) whose job aye does itself
# (parallelism, coverage, timeouts) or that only display: pytest would load
# them, and aye loading none of them changes no outcome.
ABSORBED_PLUGINS = frozenset(
    {
        "xdist",
        "xdist.looponfail",
        "pytest_cov",
        "timeout",
        "pytest_timeout",
        "sugar",
        "pytest_sugar",
        "instafail",
        "pytest_instafail",
        "pretty",
        "pytest_pretty",
        "metadata",
        "html",
    }
)
# pytest's own plugin names, which ``pytest_plugins`` and ``-p`` may name.
BUILTIN_PLUGINS = frozenset(
    {
        *PLUGIN_FIXTURES,
        *ESSENTIAL_PLUGINS,
        "python",
        "terminal",
        "debugging",
        "unittest",
        "skipping",
        "nose",
        "assertion",
        "warnings",
        "doctest",
        "pastebin",
        "setuponly",
        "setupplan",
        "stepwise",
        "faulthandler",
        "threadexception",
        "unraisableexception",
        "python_path",
        "lfplugin",
        "nfplugin",
        "pytester",
        "pytester_assertions",
        "sugar",
        "xdist",
        "randomly",
    }
)

# pytest's builtin ini keys: name -> (type, default).
INI: dict[str, tuple[str, Any]] = {
    "addopts": ("args", []),
    "minversion": ("string", None),
    "pythonpath": ("paths", []),
    "required_plugins": ("args", []),
    "assertion_text_diff_style": ("string", "ndiff"),
    "cache_dir": ("string", ".pytest_cache"),
    "collect_imported_tests": ("bool", True),
    "consider_namespace_packages": ("bool", False),
    "console_output_style": ("string", "progress"),
    "disable_test_id_escaping_and_forfeit_all_rights_to_community_support": ("bool", False),
    "doctest_encoding": ("string", "utf-8"),
    "doctest_optionflags": ("args", ["ELLIPSIS"]),
    "empty_parameter_set_mark": ("string", ""),
    "enable_assertion_pass_hook": ("bool", False),
    "faulthandler_exit_on_timeout": ("bool", False),
    "faulthandler_timeout": ("string", 0.0),
    "filterwarnings": ("linelist", []),
    "junit_duration_report": ("string", "total"),
    "junit_family": ("string", "xunit2"),
    "junit_log_passing_tests": ("bool", True),
    "junit_logging": ("string", "no"),
    "junit_suite_name": ("string", "pytest"),
    "log_auto_indent": ("string", None),
    "log_cli": ("bool", False),
    "log_cli_date_format": ("string", None),
    "log_cli_format": ("string", None),
    "log_cli_level": ("string", None),
    "log_date_format": ("string", "%H:%M:%S"),
    "log_file": ("string", None),
    "log_file_date_format": ("string", None),
    "log_file_format": ("string", None),
    "log_file_level": ("string", None),
    "log_file_mode": ("string", "w"),
    "log_format": ("string", "%(levelname)-8s %(name)s:%(filename)s:%(lineno)d %(message)s"),
    "log_level": ("string", None),
    "markers": ("linelist", []),
    "max_warnings": ("string", ""),
    "norecursedirs": (
        "args",
        ["*.egg", ".*", "_darcs", "build", "CVS", "dist", "node_modules", "venv", "{arch}"],
    ),
    "python_classes": ("args", ["Test"]),
    "python_files": ("args", ["test_*.py", "*_test.py"]),
    "python_functions": ("args", ["test"]),
    "strict": ("bool", False),
    "strict_config": ("bool", None),
    "strict_markers": ("bool", None),
    "strict_parametrization_ids": ("bool", None),
    "strict_xfail": ("bool", None),
    "testpaths": ("args", []),
    "tmp_path_retention_count": ("string", "3"),
    "tmp_path_retention_policy": ("string", "all"),
    "truncation_limit_chars": ("string", None),
    "truncation_limit_lines": ("string", None),
    "usefixtures": ("args", []),
    "verbosity_assertions": ("string", "auto"),
    "verbosity_subtests": ("string", "auto"),
    "verbosity_test_cases": ("string", "auto"),
}
INI_ALIASES = {"xfail_strict": "strict_xfail"}
# The marks pytest registers itself.
BUILTIN_MARKERS = frozenset(
    {
        "filterwarnings",
        "skip",
        "skipif",
        "xfail",
        "parametrize",
        "usefixtures",
        "tryfirst",
        "trylast",
    }
)


# While aye's own code builds what listing makes, the collector's threshold
# for its youngest generation is this, unreachable: no automatic garbage
# collection (``no_automatic_gc``).
NO_AUTOMATIC_GC = 1 << 30
_gc_collect, _gc_isenabled = gc.collect, gc.isenabled
_gc_get_threshold, _gc_set_threshold = gc.get_threshold, gc.set_threshold

# ------------------------------------------------------------------ helpers


class _Diagnostic(Exception):
    """Ends a step with a diagnostic (severity, code, path, message)."""

    def __init__(self, severity: str, code: str, path: str, message: str) -> None:
        super().__init__(message)
        self.fields = {"severity": severity, "code": code, "path": path, "message": message}


class _SkipCollect(Exception):
    """A module or directory pytest skips while collecting it."""

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


class _ConftestFailed(Exception):
    """A conftest that failed to load: ``kind`` is ``error``, ``skip`` or
    ``unsupported`` (a hook or plugin aye doesn't run, already reported)."""

    def __init__(
        self, path: str, cause: BaseException | None, kind: str, message: str = ""
    ) -> None:
        super().__init__(message or str(cause))
        self.path = path
        self.cause = cause
        self.kind = kind
        self.message = message


class _ImportMismatch(ImportError):
    pass


def strtobool(value: str) -> bool:
    """pytest's reading of a boolean: ``y yes t true on 1`` or ``n no f
    false off 0``, in any case."""
    lowered = value.lower()
    if lowered in ("y", "yes", "t", "true", "on", "1"):
        return True
    if lowered in ("n", "no", "f", "false", "off", "0"):
        return False
    raise ValueError(f"invalid truth value {value!r}")


_NON_PRINTABLE = {i: f"\\x{i:02x}" for i in range(128) if i not in range(32, 127)}
_NON_PRINTABLE.update({ord("\t"): "\\t", ord("\r"): "\\r", ord("\n"): "\\n"})


def ascii_escaped(value: str | bytes) -> str:
    """A parameter id as pytest writes it: bytes as ASCII with escapes for
    the rest, text with Python's unicode escapes, ASCII control characters
    as ``\\xNN`` (``\\t``, ``\\r`` and ``\\n`` as such)."""
    if isinstance(value, bytes):
        text = value.decode("ascii", "backslashreplace")
    elif value.isascii() and value.isprintable() and "\\" not in value:
        return value  # nothing to escape
    else:
        text = value.encode("unicode_escape").decode("ascii")
    return text.translate(_NON_PRINTABLE)


def fnmatch_ex(pattern: str, path: str | os.PathLike[str]) -> bool:
    """pytest's path glob: a pattern without a separator matches the base
    name, one with a separator the whole path (``*/`` prepended when the
    path is absolute and the pattern isn't)."""
    text = os.fspath(path)
    if sys.platform.startswith("win") and os.sep not in pattern and "/" in pattern:
        pattern = pattern.replace("/", os.sep)
    if os.sep not in pattern:
        name = os.path.basename(text)
    else:
        name = text
        if os.path.isabs(name) and not os.path.isabs(pattern):
            pattern = f"*{os.sep}{pattern}"
    return fnmatch.fnmatch(name, pattern)


def safe_getattr(obj: Any, name: str, default: Any) -> Any:
    """``getattr``, or ``default`` when it raises anything (an outcome
    exception included)."""
    try:
        return getattr(obj, name, default)
    except (*OUTCOMES,):
        return default


def get_real_func(obj: Any) -> Any:
    """The function behind ``functools.wraps`` wrappers and partials."""
    try:
        obj = inspect.unwrap(obj)
    except ValueError:
        return obj
    while isinstance(obj, functools.partial):
        obj = obj.func
    return obj


def num_mock_patch_args(function: Any) -> int:
    """How many leading arguments ``mock.patch`` decorators fill."""
    patchings = getattr(function, "patchings", None)
    if not patchings:
        return 0
    sentinels = [
        getattr(sys.modules.get(name), "DEFAULT", object()) for name in ("mock", "unittest.mock")
    ]
    return len(
        [
            p
            for p in patchings
            if not p.attribute_name and any(p.new is sentinel for sentinel in sentinels)
        ]
    )


def getfuncargnames(function: Any, *, name: str = "", cls: type | None = None) -> tuple[str, ...]:
    """The names of a function's mandatory arguments, as pytest asks
    fixtures for them: positional-or-keyword and keyword-only ones without
    defaults, the first dropped for a method of ``cls`` (not a static
    method), and those ``mock.patch`` fills."""
    try:
        parameters = list(inspect.signature(function).parameters.values())
    except (ValueError, TypeError) as exc:
        pytest.fail(f"Could not determine arguments of {function!r}: {exc}", pytrace=False)
    names = tuple(
        p.name
        for p in parameters
        if p.kind in (inspect.Parameter.POSITIONAL_OR_KEYWORD, inspect.Parameter.KEYWORD_ONLY)
        and p.default is inspect.Parameter.empty
    )
    if not name:
        name = getattr(function, "__name__", "")
    if not any(p.kind is inspect.Parameter.POSITIONAL_ONLY for p in parameters) and (
        cls is not None and not isinstance(inspect.getattr_static(cls, name, None), staticmethod)
    ):
        names = names[1:]
    if hasattr(function, "__wrapped__"):
        names = names[num_mock_patch_args(function) :]
    return names


def get_default_arg_names(function: Any) -> tuple[str, ...]:
    return tuple(
        p.name
        for p in inspect.signature(function).parameters.values()
        if p.kind in (inspect.Parameter.POSITIONAL_OR_KEYWORD, inspect.Parameter.KEYWORD_ONLY)
        and p.default is not inspect.Parameter.empty
    )


def getfslineno(obj: Any) -> tuple[str | None, int]:
    """The file and the 1-based first line of a function or class (its first
    decorator's line, as pytest's report locations)."""
    obj = get_real_func(obj)
    obj = getattr(obj, "place_as", obj)
    code = getattr(obj, "__code__", None)
    if code is not None:
        return code.co_filename, code.co_firstlineno
    try:
        return inspect.getsourcefile(obj) or inspect.getfile(obj), inspect.findsource(obj)[1] + 1
    except (TypeError, OSError):
        return None, 0


def _hook_argnames(function: Any) -> tuple[str, ...]:
    """The arguments pluggy passes a hook implementation: its positional
    ones without defaults, without ``self``."""
    func = function
    if not inspect.isroutine(func):
        func = getattr(func, "__call__", func)  # noqa: B004 - pluggy's own reading
    try:
        signature = inspect.signature(func.__func__ if inspect.ismethod(func) else func)
    except (TypeError, ValueError):
        return ()
    kinds = (inspect.Parameter.POSITIONAL_ONLY, inspect.Parameter.POSITIONAL_OR_KEYWORD)
    params = [p for p in signature.parameters.values() if p.kind in kinds]
    args = tuple(p.name for p in params if p.default is inspect.Parameter.empty)
    implicit = ("self",) if not hasattr(sys, "pypy_version_info") else ("self", "obj")
    if args and (
        inspect.ismethod(func) or ("." in getattr(func, "__qualname__", "") and args[0] in implicit)
    ):
        args = args[1:]
    return args


def _call_with_optional_argument(func: Any, arg: Any) -> None:
    """``func(arg)`` when ``func`` takes an argument, else ``func()``."""
    count = func.__code__.co_argcount
    if inspect.ismethod(func):
        count -= 1
    if count:
        func(arg)
    else:
        func()


def _fixture_spec(obj: Any) -> Any:
    spec = safe_getattr(obj, "_aye_fixture", None)
    fixture_spec = getattr(pytest, "FixtureSpec", None)
    if spec is None or (isinstance(fixture_spec, type) and not isinstance(spec, fixture_spec)):
        return None
    return spec


def _first_non_fixture(obj: Any, names: Iterable[str]) -> Any:
    """The first of ``names`` on ``obj`` that isn't a fixture (pytest's
    xunit lookup)."""
    for name in names:
        found = safe_getattr(obj, name, None)
        if found is not None and _fixture_spec(found) is None:
            return found
    return None


def _normalize_mark(mark: Any) -> Any:
    inner = getattr(mark, "mark", mark)
    if not isinstance(inner, pytest.Mark):
        raise TypeError(f"got {mark!r} instead of Mark")
    return inner


def get_unpacked_marks(obj: Any, *, consider_mro: bool = True) -> list[Any]:
    """The marks stored on ``obj`` (``pytestmark``): a class's own and its
    bases', base classes first; anything else's list or single mark."""
    if isinstance(obj, type):
        classes = reversed(obj.__mro__) if consider_mro else (obj,)
        marks: list[Any] = []
        for cls in classes:
            stored = cls.__dict__.get("pytestmark", [])
            if isinstance(stored, list):
                marks.extend(stored)
            else:
                marks.append(stored)
    else:
        stored = safe_getattr(obj, "pytestmark", [])
        if stored is None:
            marks = []
        elif isinstance(stored, list):
            marks = stored
        else:
            marks = [stored]
    return [_normalize_mark(mark) for mark in marks]


def _tag(mark: Any) -> Tag:
    return Tag(mark.name, tuple(mark.args), dict(mark.kwargs))


def _is_testcase(obj: Any) -> bool:
    try:
        return isinstance(obj, type) and issubclass(obj, unittest.TestCase)
    except Exception:  # noqa: BLE001 - pytest's own guard: no TestCase then
        return False


def _hasinit(obj: Any) -> bool:
    init = getattr(obj, "__init__", None)
    return bool(init) and init != object.__init__


def _hasnew(obj: Any) -> bool:
    new = getattr(obj, "__new__", None)
    return bool(new) and new != object.__new__


def _identifier(name: str) -> bool:
    return name.isidentifier()


def resolve_package_path(path: str) -> str | None:
    """The last directory upward from ``path`` that is still a package
    (an ``__init__.py`` and an identifier for a name), with no gap."""
    found = None
    directory = path if os.path.isdir(path) else os.path.dirname(path)
    while os.path.isfile(os.path.join(directory, "__init__.py")) and _identifier(
        os.path.basename(directory)
    ):
        found = directory
        parent = os.path.dirname(directory)
        if parent == directory:
            break
        directory = parent
    return found


def compute_module_name(root: str, path: str) -> str | None:
    names = os.path.relpath(os.path.splitext(path)[0], root).split(os.sep)
    if not names or names[0] in ("", os.curdir, os.pardir):
        return None
    if names[-1] == "__init__":
        names.pop()
    return ".".join(names) or None


def module_name_from_path(path: str, root: str) -> str:
    """pytest's unique module name for ``importlib`` mode outside packages:
    the path relative to the rootdir, dotted."""
    base = os.path.splitext(path)[0]
    try:
        parts = list(Path(base).relative_to(root).parts)
    except ValueError:
        parts = list(Path(base).parts[1:])
    if len(parts) > 1 and parts[-1] == "__init__":
        parts.pop()
    return ".".join(part.replace(".", "_") for part in parts)


def _same_file(a: str, b: str) -> bool:
    try:
        return os.path.samefile(a, b)
    except OSError:
        return os.path.normcase(os.path.abspath(a)) == os.path.normcase(os.path.abspath(b))


def _in_venv(path: str) -> bool:
    try:
        return os.path.isfile(os.path.join(path, "pyvenv.cfg")) or os.path.isfile(
            os.path.join(path, "conda-meta", "history")
        )
    except OSError:
        return False


def _short_traceback(exc: BaseException) -> str:
    """The exception and the user's frames (no importlib's, no aye's), as
    pytest's short tracebacks show them."""
    frames = []
    for frame in traceback.extract_tb(exc.__traceback__):
        name = frame.filename
        if name.startswith(("<aye:", "<frozen ")) or "importlib" in name:
            continue
        frames.append(frame)
    lines = traceback.format_list(frames)
    lines += traceback.format_exception_only(type(exc), exc)
    return "".join(lines).rstrip("\n")


# ------------------------------------------------------------------ settings


def _read_ini_file(path: str) -> tuple[dict[str, Any], bool]:  # noqa: PLR0911 - one return per case
    """The pytest section of a configuration file as raw values, and
    whether it is TOML mode (``[tool.pytest]``, native types)."""
    name = os.path.basename(path)
    if name.endswith(".toml"):
        if tomllib is None:
            return {}, False
        try:
            with open(path, "rb") as file:
                data = tomllib.load(file)
        except (OSError, ValueError):
            return {}, False
        if name in ("pytest.toml", ".pytest.toml"):
            return dict(data.get("pytest", data)), True
        tool = data.get("tool", {}).get("pytest", {})
        if "ini_options" in tool:
            return dict(tool["ini_options"]), False
        return {k: v for k, v in tool.items() if k != "ini_options"}, True
    parser = configparser.ConfigParser(interpolation=None, strict=False, delimiters=("=", ":"))
    parser.optionxform = str  # type: ignore[assignment,method-assign]
    try:
        with open(path, encoding="utf-8") as file:
            parser.read_file(file)
    except (OSError, configparser.Error, UnicodeDecodeError):
        return {}, False
    section = "tool:pytest" if name == "setup.cfg" else "pytest"
    if not parser.has_section(section):
        return {}, False
    return dict(parser.items(section)), False


@contextmanager
def no_automatic_gc() -> Iterator[None]:
    """No automatic garbage collection while aye's own code builds what
    listing makes (items, cases, their wire format): the generational
    collector would otherwise scan these long-lived objects again and again
    as they pile up. No user code runs in such a block, so none sees the
    collector's settings changed, and they are put back as they were; what
    became garbage meanwhile is collected as usual, afterwards. Within
    another such block (or with the collector set never to run), it
    changes nothing."""
    before = _gc_get_threshold()
    if before[0] in (NO_AUTOMATIC_GC, 0):
        yield
        return
    _gc_set_threshold(NO_AUTOMATIC_GC, *before[1:])
    try:
        yield
    finally:
        _gc_set_threshold(*before)


def settle_gc() -> None:
    """Once a session's files are listed: one collection of the younger
    generations (what listing made, not the rest of the heap), as the
    collector could have run at that point by itself, unless it is off (the
    session's code turned it off, or set it never to run). Listing's
    objects, which live as long as the session, then sit in the oldest
    generation, rather than being scanned again by the younger ones'
    collections while the tests run."""
    if _gc_isenabled() and _gc_get_threshold()[0]:
        _gc_collect(1)


class Settings:
    """What the front-end reads before anything is imported: the rootdir,
    the configuration file and its raw values, and the option vectors, from
    the recognizer's context (its ``native`` settings when given)."""

    def __init__(self, root: str, context: dict[str, Any]) -> None:
        native = context.get("native")
        self.native: dict[str, Any] = native if isinstance(native, dict) else {}
        rootdir = self.native.get("rootdir") or context.get("rootdir") or root
        self.rootdir = os.path.normpath(os.path.join(root, rootdir))
        inifile = self.native.get("inifile", context.get("inifile"))
        self.inifile = os.path.normpath(os.path.join(root, inifile)) if inifile else None
        raw = self.native.get("ini")
        self.toml = False
        if isinstance(raw, dict):
            self.ini: dict[str, Any] = dict(raw)
        elif self.inifile:
            self.ini, self.toml = _read_ini_file(self.inifile)
        else:
            self.ini = {}
        self.args = self._args(context)
        roots = context.get("selected_paths") or context.get("roots")
        self.roots = [os.path.normpath(os.path.join(root, r)) for r in roots or () if r]

    def _args(self, context: dict[str, Any]) -> list[str]:
        """The option vector pytest parses: ``addopts``, then
        ``PYTEST_ADDOPTS``, then the arguments after ``--``."""
        if isinstance(self.native.get("args"), list):
            return [str(arg) for arg in self.native["args"]]
        args = _strings(context.get("addopts", self.ini.get("addopts")))
        if "pytest_addopts_env" in context:
            args += _strings(context.get("pytest_addopts_env"))
        else:
            args += _strings(os.environ.get("PYTEST_ADDOPTS"))
        return args + _strings(context.get("framework_args"))


@contextmanager
def _catching_logs(handler: logging.Handler, level: int | None) -> Iterator[None]:
    """pytest's ``catching_logs``: ``handler`` on the root logger and on
    every logger that doesn't propagate, at ``level`` if given, the root's
    level lowered to it meanwhile."""
    root = logging.getLogger()
    if level is not None:
        handler.setLevel(level)
    attached = [root]
    attached += [
        logger
        for logger in list(root.manager.loggerDict.values())
        if isinstance(logger, logging.Logger) and not logger.propagate and logger is not root
    ]
    for logger in attached:
        logger.addHandler(handler)
    original = root.level
    if level is not None:
        root.setLevel(min(original, level))
    try:
        yield
    finally:
        if level is not None:
            root.setLevel(original)
        for logger in attached:
            logger.removeHandler(handler)


def _strings(value: Any) -> list[str]:
    if not value:
        return []
    if isinstance(value, str):
        try:
            return shlex.split(value)
        except ValueError:
            return value.split()
    return [str(item) for item in value]


# ------------------------------------------------------------------ options


class _OverrideIni(argparse.Action):
    """``--strict-markers`` and friends: ``True``, and an ``-o`` override."""

    def __init__(self, option_strings: Sequence[str], dest: str, **kwargs: Any) -> None:
        self.ini_option = kwargs.pop("ini_option")
        self.ini_value = kwargs.pop("ini_value", "true")
        kwargs.pop("nargs", None)
        super().__init__(option_strings, dest, 0, **kwargs)

    def __call__(self, parser: Any, namespace: Any, values: Any, option_string: Any = None) -> None:
        setattr(namespace, self.dest, True)
        overrides = list(getattr(namespace, "override_ini", None) or [])
        overrides.append(f"{self.ini_option}={self.ini_value}")
        namespace.override_ini = overrides


class _Decrement(argparse.Action):
    """``-q``: one less verbosity."""

    def __init__(self, option_strings: Sequence[str], dest: str, **kwargs: Any) -> None:
        kwargs.pop("nargs", None)
        super().__init__(option_strings, dest, 0, **kwargs)

    def __call__(self, parser: Any, namespace: Any, values: Any, option_string: Any = None) -> None:
        setattr(namespace, self.dest, (getattr(namespace, self.dest, 0) or 0) - 1)


_TRUE = {"action": "store_true", "default": False}

# pytest 9.1.1's builtin options: names -> argparse attributes.
BUILTIN_OPTIONS: tuple[tuple[tuple[str, ...], dict[str, Any]], ...] = (
    (("-k",), {"dest": "keyword", "default": ""}),
    (("-m",), {"dest": "markexpr", "default": ""}),
    (("--markers",), {**_TRUE, "dest": "markers"}),
    (("-x", "--exitfirst"), {"action": "store_const", "dest": "maxfail", "const": 1, "default": 0}),
    (("--maxfail",), {"dest": "maxfail", "type": int, "default": 0}),
    (("--strict-config",), {"action": _OverrideIni, "ini_option": "strict_config"}),
    (("--strict-markers",), {"action": _OverrideIni, "ini_option": "strict_markers"}),
    (("--strict",), {"action": _OverrideIni, "ini_option": "strict"}),
    (("--fixtures", "--funcargs"), {**_TRUE, "dest": "showfixtures"}),
    (("--fixtures-per-test",), {**_TRUE, "dest": "show_fixtures_per_test"}),
    (("--pdb",), {**_TRUE, "dest": "usepdb"}),
    (("--pdbcls",), {"dest": "usepdb_cls", "default": None}),
    (("--trace",), {**_TRUE, "dest": "trace"}),
    (("--capture",), {"dest": "capture", "default": "fd"}),
    (("-s",), {"action": "store_const", "dest": "capture", "const": "no"}),
    (("--runxfail",), {**_TRUE, "dest": "runxfail"}),
    (("--lf", "--last-failed"), {**_TRUE, "dest": "lf"}),
    (("--ff", "--failed-first"), {**_TRUE, "dest": "failedfirst"}),
    (("--nf", "--new-first"), {**_TRUE, "dest": "newfirst"}),
    (("--cache-show",), {"dest": "cacheshow", "nargs": "?", "default": None}),
    (("--cache-clear",), {**_TRUE, "dest": "cacheclear"}),
    (
        ("--lfnf", "--last-failed-no-failures"),
        {"dest": "last_failed_no_failures", "default": "all"},
    ),
    (("--sw", "--stepwise"), {**_TRUE, "dest": "stepwise"}),
    (("--sw-skip", "--stepwise-skip"), {**_TRUE, "dest": "stepwise_skip"}),
    (("--sw-reset", "--stepwise-reset"), {**_TRUE, "dest": "stepwise_reset"}),
    (("--durations",), {"dest": "durations", "type": int, "default": None}),
    (("--durations-min",), {"dest": "durations_min", "type": float, "default": None}),
    (("-v", "--verbose"), {"action": "count", "dest": "verbose", "default": 0}),
    (("--no-header",), {**_TRUE, "dest": "no_header"}),
    (("--no-summary",), {**_TRUE, "dest": "no_summary"}),
    (("--no-fold-skipped",), {"action": "store_false", "dest": "fold_skipped", "default": True}),
    (("--force-short-summary",), {**_TRUE, "dest": "force_short_summary"}),
    (("-q", "--quiet"), {"action": _Decrement, "dest": "verbose", "default": 0}),
    (("--verbosity",), {"dest": "verbose", "type": int, "default": 0}),
    (("-r", "--report-chars"), {"dest": "reportchars", "default": "fE"}),
    (("--disable-warnings", "--disable-pytest-warnings"), {**_TRUE, "dest": "disable_warnings"}),
    (("-l", "--showlocals"), {**_TRUE, "dest": "showlocals"}),
    (("--no-showlocals",), {"action": "store_false", "dest": "showlocals"}),
    (("--tb",), {"dest": "tbstyle", "default": "auto"}),
    (("--xfail-tb",), {**_TRUE, "dest": "xfail_tb"}),
    (("--show-capture",), {"dest": "showcapture", "default": "all"}),
    (("--fulltrace", "--full-trace"), {**_TRUE, "dest": "fulltrace"}),
    (("--color",), {"dest": "color", "default": "auto"}),
    (("--code-highlight",), {"dest": "code_highlight", "default": "yes"}),
    (("--pastebin",), {"dest": "pastebin", "default": None}),
    (("--junitxml", "--junit-xml"), {"dest": "xmlpath", "default": None}),
    (("--junitprefix", "--junit-prefix"), {"dest": "junitprefix", "default": None}),
    (("-W", "--pythonwarnings"), {"action": "append", "dest": "pythonwarnings", "default": None}),
    (("--max-warnings",), {"dest": "max_warnings", "type": int, "default": None}),
    (("--collectonly", "--collect-only", "--co"), {**_TRUE, "dest": "collectonly"}),
    (("--pyargs",), {**_TRUE, "dest": "pyargs"}),
    (("--ignore",), {"action": "append", "dest": "ignore", "default": None}),
    (("--ignore-glob",), {"action": "append", "dest": "ignore_glob", "default": None}),
    (("--deselect",), {"action": "append", "dest": "deselect", "default": None}),
    (("--confcutdir",), {"dest": "confcutdir", "default": None}),
    (("--noconftest",), {**_TRUE, "dest": "noconftest"}),
    (("--keepduplicates", "--keep-duplicates"), {**_TRUE, "dest": "keepduplicates"}),
    (("--collect-in-virtualenv",), {**_TRUE, "dest": "collect_in_virtualenv"}),
    (("--continue-on-collection-errors",), {**_TRUE, "dest": "continue_on_collection_errors"}),
    (("--import-mode",), {"dest": "importmode", "default": "prepend"}),
    (("--doctest-modules",), {**_TRUE, "dest": "doctestmodules"}),
    (("--doctest-report",), {"dest": "doctestreport", "default": "udiff"}),
    (("--doctest-glob",), {"action": "append", "dest": "doctestglob", "default": []}),
    (("--doctest-ignore-import-errors",), {**_TRUE, "dest": "doctest_ignore_import_errors"}),
    (("--doctest-continue-on-failure",), {**_TRUE, "dest": "doctest_continue_on_failure"}),
    (("-c", "--config-file"), {"dest": "inifilename", "default": None}),
    (("--rootdir",), {"dest": "rootdir", "default": None}),
    (("--basetemp",), {"dest": "basetemp", "default": None}),
    (("-V", "--version"), {"action": "count", "dest": "version", "default": 0}),
    (("-h", "--help"), {**_TRUE, "dest": "help"}),
    (("-p",), {"action": "append", "dest": "plugins", "default": []}),
    (("--disable-plugin-autoload",), {**_TRUE, "dest": "disable_plugin_autoload"}),
    (("--traceconfig", "--trace-config"), {**_TRUE, "dest": "traceconfig"}),
    (("--debug",), {"dest": "debug", "nargs": "?", "const": "pytestdebug.log", "default": None}),
    (("-o", "--override-ini"), {"action": "append", "dest": "override_ini", "default": None}),
    (("--assert",), {"dest": "assertmode", "default": "rewrite"}),
    (("--setuponly", "--setup-only"), {**_TRUE, "dest": "setuponly"}),
    (("--setupshow", "--setup-show"), {**_TRUE, "dest": "setupshow"}),
    (("--setupplan", "--setup-plan"), {**_TRUE, "dest": "setupplan"}),
    (("--log-level",), {"dest": "log_level", "default": None}),
    (("--log-format",), {"dest": "log_format", "default": None}),
    (("--log-date-format",), {"dest": "log_date_format", "default": None}),
    (("--log-cli-level",), {"dest": "log_cli_level", "default": None}),
    (("--log-cli-format",), {"dest": "log_cli_format", "default": None}),
    (("--log-cli-date-format",), {"dest": "log_cli_date_format", "default": None}),
    (("--log-file",), {"dest": "log_file", "default": None}),
    (("--log-file-mode",), {"dest": "log_file_mode", "default": None}),
    (("--log-file-level",), {"dest": "log_file_level", "default": None}),
    (("--log-file-format",), {"dest": "log_file_format", "default": None}),
    (("--log-file-date-format",), {"dest": "log_file_date_format", "default": None}),
    (("--log-auto-indent",), {"dest": "log_auto_indent", "default": None}),
    (("--log-disable",), {"action": "append", "dest": "logger_disable", "default": []}),
)


class _Option:
    """One option a parser holds: its names and argparse attributes."""

    __slots__ = ("attrs", "names")

    def __init__(self, names: Sequence[str], attrs: dict[str, Any]) -> None:
        self.names = tuple(names)
        self.attrs = dict(attrs)

    @property
    def dest(self) -> str:
        dest = self.attrs.get("dest")
        if dest:
            return str(dest)
        longs = [n for n in self.names if n.startswith("--")]
        name = longs[0][2:] if longs else self.names[0].lstrip("-")
        return name.replace("-", "_")


class OptionGroup(pytest.OptionGroup):
    """``parser.getgroup(...)``: options added together."""

    def __init__(self, name: str, description: str = "", parser: Parser | None = None) -> None:
        self.name = name
        self.description = description
        self.options: list[_Option] = []
        self.parser = parser

    def addoption(self, *opts: str, **attrs: Any) -> None:
        """Add an option, as argparse's ``add_argument``. A lowercase short
        option is reserved for pytest."""
        taken = {name for option in self.options for name in option.names}
        conflict = set(opts) & taken
        if conflict:
            raise ValueError(f"option names {conflict} already added")
        for opt in opts:
            if len(opt) == 2 and opt[0] == "-" and opt[1].islower():  # noqa: PLR2004
                raise ValueError("lowercase shortoptions reserved")
        self._add(opts, attrs)

    def _addoption(self, *opts: str, **attrs: Any) -> None:
        self._add(opts, attrs)

    def _add(self, opts: Sequence[str], attrs: dict[str, Any]) -> None:
        option = _Option(opts, attrs)
        self.options.append(option)
        if self.parser is not None:
            self.parser.added(option)


class Parser(pytest.Parser):
    """The ``parser`` ``pytest_addoption`` receives: conftests' options and
    ini keys."""

    def __init__(self, frontend: Frontend) -> None:
        self._frontend = frontend
        self._anonymous = OptionGroup("Custom options", parser=self)
        self._groups: list[OptionGroup] = []
        self.options: list[_Option] = []

    def getgroup(self, name: str, description: str = "", after: str | None = None) -> OptionGroup:
        for group in self._groups:
            if group.name == name:
                return group
        group = OptionGroup(name, description, parser=self)
        self._groups.append(group)
        return group

    def addoption(self, *opts: str, **attrs: Any) -> None:
        self._anonymous.addoption(*opts, **attrs)

    def addini(
        self,
        name: str,
        help: str,
        type: str | None = None,
        default: Any = NOTSET,
        *,
        aliases: Sequence[str] = (),
    ) -> None:
        """Register an ini key. Its default, when not given, is the type's:
        ``[]`` for lists, ``False``, ``""``, ``0`` or ``0.0``."""
        kind = type or "string"
        if default is NOTSET:
            default = {
                "paths": [],
                "pathlist": [],
                "args": [],
                "linelist": [],
                "bool": False,
                "int": 0,
                "float": 0.0,
            }.get(kind, "")
        self._frontend.config.ini_types[name] = (kind, default)
        for alias in aliases:
            self._frontend.config.ini_aliases[alias] = name

    def added(self, option: _Option) -> None:
        self.options.append(option)
        self._frontend.config.option_added(option)


# ------------------------------------------------------------------ hooks


class HookImpl:
    """One implementation of a hook: the function, the arguments it takes
    and pluggy's ordering options."""

    __slots__ = ("argnames", "function", "name", "plugin", "tryfirst", "trylast")

    def __init__(self, name: str, function: Any, plugin: Any, opts: dict[str, Any]) -> None:
        self.name = name
        self.function = function
        self.plugin = plugin
        self.argnames = _hook_argnames(function)
        self.tryfirst = bool(opts.get("tryfirst"))
        self.trylast = bool(opts.get("trylast"))


def _insert(impls: list[HookImpl], impl: HookImpl) -> None:
    """pluggy's order: called last in, first out, ``tryfirst`` ones before
    the rest and ``trylast`` ones after."""
    if impl.trylast:
        impls.insert(0, impl)
    elif impl.tryfirst:
        impls.append(impl)
    else:
        index = len(impls) - 1
        while index >= 0 and impls[index].tryfirst:
            index -= 1
        impls.insert(index + 1, impl)


class PluginManager(pytest.PytestPluginManager):
    """``config.pluginmanager``: the plugins (conftests, ``pytest_plugins``
    modules, objects registered in ``pytest_configure``) and their hooks."""

    def __init__(self, frontend: Frontend) -> None:
        self._frontend = frontend
        self._plugins: dict[str, Any] = {}
        self._names: dict[int, str] = {}
        self._impls: dict[str, list[HookImpl]] = {}
        # Conftest directories by plugin, for path-sensitive calls.
        self._directories: dict[int, str] = {}
        self._blocked: set[str] = set()
        # The implementations a call reaches, by hook and directory, while
        # no plugin comes or goes.
        self._reached: dict[tuple[str, str | None], list[HookImpl]] = {}

    # -- registration

    def add(self, plugin: Any, name: str, impls: list[HookImpl], directory: str | None) -> None:
        self._reached.clear()
        self._plugins[name] = plugin
        self._names[id(plugin)] = name
        if directory is not None:
            self._directories[id(plugin)] = directory
        for impl in impls:
            _insert(self._impls.setdefault(impl.name, []), impl)

    def register(self, plugin: Any, name: str | None = None) -> str | None:
        """Register a plugin object (from a conftest's ``pytest_configure``):
        its hooks, as conftests' are checked, and its fixtures, visible
        everywhere."""
        name = name or str(id(plugin))
        if name in self._blocked or name in self._plugins:
            return None
        self._frontend.register_plugin(plugin, name, f"plugin {name!r}")
        return name

    def unregister(self, plugin: Any = None, name: str | None = None) -> Any:
        name = name or self._names.get(id(plugin))
        if name is None:
            return None
        plugin = self._plugins.pop(name, None)
        self._names.pop(id(plugin), None)
        self._reached.clear()
        for impls in self._impls.values():
            impls[:] = [impl for impl in impls if impl.plugin is not plugin]
        return plugin

    def set_blocked(self, name: str) -> None:
        self._blocked.add(name)
        self.unregister(name=name)

    def is_blocked(self, name: str) -> bool:
        return name in self._blocked

    def get_plugin(self, name: str) -> Any:
        return self._plugins.get(name)

    getplugin = get_plugin

    def has_plugin(self, name: str) -> bool:
        return name in self._plugins

    hasplugin = has_plugin

    def is_registered(self, plugin: Any) -> bool:
        return id(plugin) in self._names

    def get_name(self, plugin: Any) -> str | None:
        return self._names.get(id(plugin))

    def get_plugins(self) -> set[Any]:
        return {plugin for plugin in self._plugins.values() if _hashable(plugin)}

    def list_name_plugin(self) -> list[tuple[str, Any]]:
        return list(self._plugins.items())

    def import_plugin(self, modname: str, consider_entry_points: bool = False) -> None:
        self._frontend.import_plugin(modname, "pluginmanager.import_plugin")

    def consider_module(self, mod: types.ModuleType) -> None:
        self._frontend.consider_module(mod, getattr(mod, "__name__", "module"))

    @property
    def hook(self) -> HookRelay:
        return self._frontend.config.hook

    # -- calls

    def has(self, name: str) -> bool:
        """Whether a plugin implements ``name``."""
        return bool(self._impls.get(name))

    def impls(self, name: str, extra: Sequence[Any] = ()) -> list[HookImpl]:
        impls = list(self._impls.get(name, ()))
        for function in extra:
            _insert(impls, HookImpl(name, function, None, {}))
        return impls

    def _reaching(self, name: str, directory: str | None, extra: Sequence[Any]) -> list[HookImpl]:
        """The implementations a call of ``name`` for ``directory`` reaches,
        in calling order (a snapshot: a plugin registered by one of them
        isn't called this time)."""
        if extra:
            return [i for i in reversed(self.impls(name, extra)) if self.applies(i, directory)]
        key = (name, directory)
        found = self._reached.get(key)
        if found is None:
            impls = self._impls.get(name, ())
            found = self._reached[key] = [i for i in reversed(impls) if self.applies(i, directory)]
        return found

    def applies(self, impl: HookImpl, directory: str | None) -> bool:
        """Whether a path-sensitive call for ``directory`` reaches ``impl``:
        a conftest's only when it lies on the directory's chain."""
        if directory is None:
            return True
        conftest = self._directories.get(id(impl.plugin))
        if conftest is None:
            return True
        return directory == conftest or directory.startswith(conftest.rstrip(os.sep) + os.sep)

    def call(
        self,
        name: str,
        kwargs: dict[str, Any],
        *,
        directory: str | None = None,
        extra: Sequence[Any] = (),
    ) -> Any:
        """Call a hook as pluggy does: last registered first, only the
        arguments each implementation takes; a ``firstresult`` hook stops at
        the first result that isn't ``None``."""
        firstresult = name in FIRSTRESULT
        results = []
        for impl in self._reaching(name, directory, extra):
            try:
                args = [kwargs[arg] for arg in impl.argnames]
            except KeyError as exc:
                raise TypeError(f"{name} got no argument {exc.args[0]!r}") from None
            result = impl.function(*args)
            if result is not None:
                results.append(result)
                if firstresult:
                    break
        if firstresult:
            return results[0] if results else None
        return results


def _hashable(value: Any) -> bool:
    try:
        hash(value)
    except TypeError:
        return False
    return True


class HookCaller:
    """``config.hook.NAME``: calls every implementation of NAME."""

    def __init__(self, manager: PluginManager, name: str, directory: str | None = None) -> None:
        self._manager = manager
        self.name = name
        self._directory = directory

    def __call__(self, **kwargs: Any) -> Any:
        return self._manager.call(self.name, kwargs, directory=self._directory)

    def call_historic(self, result_callback: Any = None, kwargs: Any = None) -> None:
        for result in self._manager.call(self.name, dict(kwargs or {}), directory=self._directory):
            if result_callback is not None:
                result_callback(result)

    def call_extra(self, methods: Sequence[Any], kwargs: dict[str, Any]) -> Any:
        return self._manager.call(self.name, kwargs, directory=self._directory, extra=methods)

    def get_hookimpls(self) -> list[HookImpl]:
        return self._manager.impls(self.name)


class HookRelay:
    """``config.hook`` (every plugin) or a node's ``ihook`` (the conftests
    on its directory's chain, and every other plugin)."""

    def __init__(self, manager: PluginManager, directory: str | None = None) -> None:
        self._manager = manager
        self._directory = directory

    def __getattr__(self, name: str) -> HookCaller:
        if not name.startswith("pytest_"):
            raise AttributeError(name)
        return HookCaller(self._manager, name, self._directory)


# ------------------------------------------------------------------ config


class InvocationParams(NamedTuple):
    args: tuple[str, ...]
    plugins: Any
    dir: Path


class _Cache(pytest.Cache):
    """``config.cache``: values kept for the batch only (aye keeps its own
    history), directories under a temporary one."""

    def __init__(self) -> None:
        self._values: dict[str, Any] = {}
        self._root: Path | None = None

    def get(self, key: str, default: Any) -> Any:
        return self._values.get(key, default)

    def set(self, key: str, value: Any) -> None:
        self._values[key] = value

    def mkdir(self, name: str) -> Path:
        if self._root is None:
            self._root = Path(tempfile.mkdtemp(prefix="aye-cache-"))
        path = self._root / name
        path.mkdir(exist_ok=True)
        return path

    makedir = mkdir


class Config(pytest.Config):
    """``pytest.Config`` for the batch: options, ini values, paths, the
    plugin manager and hooks."""

    def __init__(self, frontend: Frontend) -> None:
        settings = frontend.settings
        self._frontend = frontend
        self.pluginmanager = PluginManager(frontend)
        self.hook = HookRelay(self.pluginmanager)
        self.option = argparse.Namespace()
        self.known_args_namespace = self.option
        self.stash = pytest.Stash()
        # pytest's older name for it, which plugins and conftests still use.
        self._store = self.stash
        self.cache = _Cache()
        self.rootpath = Path(settings.rootdir)
        self.inipath = Path(settings.inifile) if settings.inifile else None
        self.invocation_params = InvocationParams(
            tuple(settings.args), None, Path(frontend.invocation_dir)
        )
        self.args: list[str] = list(settings.roots)
        self.args_source = "args"
        self.ini_types: dict[str, tuple[str, Any]] = dict(INI)
        self.ini_aliases: dict[str, str] = dict(INI_ALIASES)
        self._inicache: dict[str, Any] = {}
        self._overrides: dict[str, str] = {}
        self._cleanups: list[Callable[[], Any]] = []
        self._parsed = False

    # -- paths

    @property
    def rootdir(self) -> Path:
        return self.rootpath

    @property
    def inifile(self) -> Path | None:
        return self.inipath

    @property
    def invocation_dir(self) -> Path:
        return self.invocation_params.dir

    # -- options

    def option_added(self, option: _Option) -> None:
        """An option a conftest added: its default, unless parsing gave it a
        value (options of conftests met while collecting keep their
        defaults, as in pytest)."""
        for name in option.names:
            self._frontend.option_dests.setdefault(name, option.dest)
        if self._parsed and not hasattr(self.option, option.dest):
            setattr(self.option, option.dest, option.attrs.get("default"))

    def getoption(self, name: str, default: Any = NOTSET, skip: bool = False) -> Any:
        """A command line option's value; ``name`` may be its dest or its
        ``--long-name``."""
        dest = self._frontend.option_dest(name)
        try:
            value = getattr(self.option, dest)
        except AttributeError:
            if default is not NOTSET:
                return default
            if skip:
                pytest.skip(f"no {name!r} option found")
            raise ValueError(f"no option named {name!r}") from None
        if value is None and skip:
            pytest.skip(f"no {name!r} value found")
        return value

    def getvalue(self, name: str, path: Any = None) -> Any:
        return self.getoption(name)

    def getvalueorskip(self, name: str, path: Any = None) -> Any:
        return self.getoption(name, skip=True)

    def get_verbosity(self, verbosity_type: str | None = None) -> int:
        global_level = int(self.getoption("verbose", 0) or 0)
        if verbosity_type is None:
            return global_level
        value = self.getini(f"verbosity_{verbosity_type}") if verbosity_type else "auto"
        return global_level if value in (None, "", "auto") else int(value)

    # -- ini

    def getini(self, name: str) -> Any:
        """An ini value, typed as its key was registered; unregistered keys
        raise ``ValueError`` as pytest's."""
        canonical = self.ini_aliases.get(name, name)
        if canonical in self._inicache:
            return self._inicache[canonical]
        if canonical not in self.ini_types:
            raise ValueError(f"unknown configuration value: {name!r}")
        value = self._getini(canonical, name)
        self._inicache[canonical] = value
        return value

    def _getini(self, canonical: str, name: str) -> Any:
        kind, default = self.ini_types[canonical]
        native = self._frontend.settings.native
        if canonical in self._overrides:
            raw: Any = self._overrides[canonical]
        elif canonical in _NATIVE_TYPED and canonical in native and native[canonical] is not None:
            return _copy(native[canonical])
        else:
            ini = self._frontend.settings.ini
            found = [key for key in (canonical, name, *self._aliases_of(canonical)) if key in ini]
            if not found:
                return _copy(default)
            raw = ini[found[0]]
        return self._coerce(canonical, kind, raw)

    def _aliases_of(self, canonical: str) -> list[str]:
        return [alias for alias, target in self.ini_aliases.items() if target == canonical]

    def _coerce(self, name: str, kind: str, raw: Any) -> Any:  # noqa: PLR0911 - one return per type
        base = self.inipath.parent if self.inipath is not None else self.invocation_params.dir
        if kind in ("paths", "pathlist"):
            values = shlex.split(raw) if isinstance(raw, str) else list(raw)
            return [base / value for value in values]
        if kind == "args":
            return shlex.split(raw) if isinstance(raw, str) else list(raw)
        if kind == "linelist":
            if isinstance(raw, str):
                return [line.strip() for line in raw.split("\n") if line.strip()]
            return list(raw)
        if kind == "bool":
            return strtobool(str(raw).strip()) if not isinstance(raw, bool) else raw
        if kind == "int":
            return int(raw)
        if kind == "float":
            return float(raw)
        return raw

    def addinivalue_line(self, name: str, line: str) -> None:
        """Add a line to a list ini value (``markers``)."""
        values = self.getini(name)
        if not isinstance(values, list):
            raise TypeError(f"{name} is not a list value")
        values.append(line)

    def set_overrides(self, overrides: Iterable[str]) -> None:
        for override in overrides:
            key, equals, value = override.partition("=")
            if not equals:
                raise pytest.UsageError(
                    f"-o/--override-ini expects option=value style (got: {override!r})."
                )
            self._overrides[self.ini_aliases.get(key, key)] = value
        self._inicache.clear()

    @property
    def inicfg(self) -> dict[str, Any]:
        """pytest's deprecated raw ini values (``-o`` overrides included),
        with its deprecation warning."""
        warnings.warn(
            pytest.PytestRemovedIn10Warning(
                "config.inicfg is deprecated, use config.getini() to access configuration "
                "values instead.\nSee https://docs.pytest.org/en/stable/deprecations.html"
                "#config-inicfg"
            ),
            stacklevel=2,
        )
        return {**self._frontend.settings.ini, **self._overrides}

    # -- the rest

    def add_cleanup(self, func: Callable[[], Any]) -> None:
        self._cleanups.append(func)

    def issue_config_time_warning(self, warning: Warning, stacklevel: int) -> None:
        warnings.warn(warning, stacklevel=stacklevel)

    def get_terminal_writer(self) -> Any:
        raise NotImplementedError("aye renders the run: there is no terminal writer")

    @property
    def workerinput(self) -> Any:  # pragma: no cover - documented absence
        raise AttributeError("workerinput")


# Ini keys the recognizer's ``native`` settings give already typed.
_NATIVE_TYPED = frozenset(
    {
        "python_files",
        "python_classes",
        "python_functions",
        "testpaths",
        "norecursedirs",
        "usefixtures",
        "filterwarnings",
        "markers",
        "empty_parameter_set_mark",
        "consider_namespace_packages",
        "collect_imported_tests",
        "strict_parametrization_ids",
    }
)


def _copy(value: Any) -> Any:
    return list(value) if isinstance(value, list) else value


# ------------------------------------------------------------------ nodes

# The directness of a test's parameters when it has none (never changed).
_NO_DIRECTNESS: dict[str, str] = {}


class NodeKeywords(MutableMapping[str, Any]):
    """A node's keywords, falling back to its parent's."""

    def __init__(self, node: NodeBase) -> None:
        self.node = node
        self.parent = node.parent
        self._markers: dict[str, Any] = {node.name: True}

    def __getitem__(self, key: str) -> Any:
        try:
            return self._markers[key]
        except KeyError:
            if self.parent is None:
                raise
            return self.parent.keywords[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self._markers[key] = value

    def __contains__(self, key: object) -> bool:
        return key in self._markers or (self.parent is not None and key in self.parent.keywords)

    def __delitem__(self, key: str) -> None:
        raise ValueError("cannot delete key in keywords dict")

    def __iter__(self) -> Iterator[str]:
        yield from self._markers
        if self.parent is not None:
            for key in self.parent.keywords:
                if key not in self._markers:
                    yield key

    def __len__(self) -> int:
        return sum(1 for _ in self)

    def __repr__(self) -> str:
        return f"<NodeKeywords for node {self.node}>"


class NodeBase:
    """What every node has: names, ids, marks, keywords, the chain.

    What most nodes never use (``stash``, ``extra_keyword_matches``, the
    finalizers) is made when first asked for: a suite's tens of thousands of
    items cost less to make, to scan for garbage and to free."""

    # The model node kind ("package", "module", "class"), when the node is one.
    model_kind: ClassVar[str | None] = None
    # Until made: no stash, no extra keyword, no finalizer.
    _aye_stash: Any = None
    _aye_extra_keywords: set[str] | None = None
    _finalizers: list[Callable[[], Any]] | tuple[()] = ()

    def _init_node(
        self, frontend: Frontend, name: str, parent: NodeBase | None, path: str | None
    ) -> None:
        self._frontend = frontend
        self.name = name
        self.parent = parent
        self.config = frontend.config
        self.session = parent.session if parent is not None else self
        self.path = Path(path) if path is not None else (parent.path if parent else None)
        self.own_markers: list[Any] = []
        self.keywords = NodeKeywords(self)

    @property
    def stash(self) -> Any:
        found = self._aye_stash
        if found is None:
            found = self._aye_stash = pytest.Stash()
        return found

    @stash.setter
    def stash(self, value: Any) -> None:
        self._aye_stash = value

    # pytest's older name for it, which plugins and conftests still use.
    _store = stash

    @property
    def extra_keyword_matches(self) -> set[str]:
        found = self._aye_extra_keywords
        if found is None:
            found = self._aye_extra_keywords = set()
        return found

    @extra_keyword_matches.setter
    def extra_keyword_matches(self, value: set[str]) -> None:
        self._aye_extra_keywords = value

    # pytest's legacy py.path attribute: a ``Path`` here.
    @property
    def fspath(self) -> Path | None:
        return self.path

    @property
    def module_path(self) -> Path | None:
        """The file of the module the node lies in (pytest's ``item.path``)."""
        return self.path

    @property
    def ihook(self) -> HookRelay:
        directory = None
        if self.path is not None:
            text = os.fspath(self.path)
            directory = text if isinstance(self, DirectoryBase) else os.path.dirname(text)
        return HookRelay(self.config.pluginmanager, directory)

    def iter_parents(self) -> Iterator[NodeBase]:
        node: NodeBase | None = self
        while node is not None:
            yield node
            node = node.parent

    def listchain(self) -> list[NodeBase]:
        return list(self.iter_parents())[::-1]

    def listnames(self) -> list[str]:
        return [node.name for node in self.listchain()]

    def getparent(self, cls: type) -> Any:
        for node in self.iter_parents():
            if isinstance(node, cls):
                return node
        return None

    def add_marker(self, marker: Any, append: bool = True) -> None:
        """Add a mark (a ``MarkDecorator``, or a name) to the node."""
        if isinstance(marker, str):
            decorator = getattr(pytest.mark, marker)
        elif isinstance(marker, pytest.MarkDecorator):
            decorator = marker
        else:
            raise ValueError("is not a string or pytest.mark.* Marker")  # noqa: TRY004 - pytest's
        self.keywords[decorator.name] = decorator
        if append:
            self.own_markers.append(decorator.mark)
        else:
            self.own_markers.insert(0, decorator.mark)
        self._frontend.marks_changed()

    def iter_markers(self, name: str | None = None) -> Iterator[Any]:
        node: NodeBase | None = self
        while node is not None:
            for mark in node.own_markers:
                if name is None or getattr(mark, "name", None) == name:
                    yield mark
            node = node.parent

    def iter_markers_with_node(self, name: str | None = None) -> Iterator[tuple[NodeBase, Any]]:
        node: NodeBase | None = self
        while node is not None:
            for mark in node.own_markers:
                if name is None or getattr(mark, "name", None) == name:
                    yield node, mark
            node = node.parent

    def get_closest_marker(self, name: str, default: Any = None) -> Any:
        return next(self.iter_markers(name), default)

    def listextrakeywords(self) -> set[str]:
        extra: set[str] = set()
        for node in self.iter_parents():
            extra.update(node._aye_extra_keywords or ())
        return extra

    def addfinalizer(self, fin: Callable[[], Any]) -> None:
        if not isinstance(self._finalizers, list):
            self._finalizers = []
        self._finalizers.append(fin)

    def warn(self, warning: Warning) -> None:
        path, line = self._frontend.location_of(self)
        warnings.warn_explicit(warning, category=None, filename=str(path), lineno=line or 1)

    def __repr__(self) -> str:
        return f"<{type(self).__name__} {self.name}>"


class Session(NodeBase, pytest.Session):
    """The root of the collection tree."""

    def __init__(self, frontend: Frontend) -> None:
        self._init_node(frontend, "", None, frontend.settings.rootdir)
        self.nodeid = ""
        self.aye_id = ""
        self.items: list[Any] = []
        self.testsfailed = 0
        self.testscollected = 0
        self.shouldstop: bool | str = False
        self.shouldfail: bool | str = False
        self.exitstatus: int | None = None

    @property
    def startpath(self) -> Path:
        return self.config.invocation_params.dir

    startdir = startpath

    @property
    def ihook(self) -> HookRelay:
        return self.config.hook

    def isinitpath(self, path: Any, *, with_parents: bool = False) -> bool:
        return self._frontend.is_initial(os.fspath(path), with_parents=with_parents)

    def gethookproxy(self, fspath: Any) -> HookRelay:
        text = os.fspath(fspath)
        directory = text if os.path.isdir(text) else os.path.dirname(text)
        return HookRelay(self.config.pluginmanager, directory)

    def perform_collect(self, *args: Any, **kwargs: Any) -> Any:
        raise pytest.UsageError("aye collects the session: a hook can't collect it again")


class DirectoryBase(NodeBase):
    """A directory's node: its conftest's fixtures are visible below it."""

    def _init_dir(self, frontend: Frontend, parent: NodeBase, path: str) -> None:
        parent_path = os.fspath(parent.path) if parent.path is not None else None
        if parent_path and path.startswith(parent_path.rstrip(os.sep) + os.sep):
            name = os.path.relpath(path, parent_path).replace(os.sep, "/")
        else:
            name = os.path.basename(path)
        self._init_node(frontend, name, parent, path)
        self.nodeid = frontend.nodeid_of_path(path)
        self.aye_id = frontend.relative(path)
        if self.aye_id == ".":
            self.aye_id = ""

    @property
    def baseid(self) -> str:
        """The id prefix of everything below: ``""`` at aye's root."""
        if not self.aye_id or self.aye_id.startswith("../"):
            return ""
        return f"{self.aye_id}/"


class Dir(DirectoryBase, pytest.Dir):
    def __init__(self, frontend: Frontend, parent: NodeBase, path: str) -> None:
        self._init_dir(frontend, parent, path)


class Package(DirectoryBase, pytest.Package):
    model_kind = "package"

    def __init__(self, frontend: Frontend, parent: NodeBase, path: str) -> None:
        self._init_dir(frontend, parent, path)
        self.obj: Any = None


class Module(NodeBase, pytest.Module):
    model_kind = "module"

    def __init__(self, frontend: Frontend, parent: NodeBase, path: str) -> None:
        parent_path = os.fspath(parent.path) if parent.path is not None else ""
        name = (
            os.path.relpath(path, parent_path).replace(os.sep, "/")
            if parent_path and path.startswith(parent_path.rstrip(os.sep) + os.sep)
            else os.path.basename(path)
        )
        self._init_node(frontend, name, parent, path)
        self.nodeid = frontend.nodeid_of_path(path)
        self.aye_id = frontend.relative(path)
        self.obj: Any = None

    @property
    def baseid(self) -> str:
        return self.aye_id

    @property
    def module(self) -> Any:
        return self.obj


class Class(NodeBase, pytest.Class):
    model_kind = "class"

    def __init__(self, frontend: Frontend, parent: NodeBase, name: str, obj: type) -> None:
        self._init_node(frontend, name, parent, None)
        self.nodeid = f"{parent.nodeid}::{name}"
        self.aye_id = f"{parent.aye_id}::{name}"  # type: ignore[attr-defined]
        self.obj = obj

    @property
    def baseid(self) -> str:
        return self.aye_id

    @property
    def module(self) -> Any:
        node = self.getparent(Module)
        return node.obj if node is not None else None

    @property
    def cls(self) -> type:
        return self.obj

    def newinstance(self) -> Any:
        return self.obj()


class UnitTestCase(Class):
    nofuncargs = True

    def newinstance(self) -> Any:
        return self.obj("runTest")


class CallSpec2:
    """One planned invocation of a test function: its parameters."""

    def __init__(
        self,
        params: dict[str, Any] | None = None,
        indices: dict[str, int] | None = None,
        arg2scope: dict[str, Scope] | None = None,
        idlist: Sequence[str] = (),
        marks: Sequence[Any] = (),
        ids: dict[str, str] | None = None,
    ) -> None:
        self.params = params or {}
        self.indices = indices or {}
        self._arg2scope = arg2scope or {}
        self._idlist = list(idlist)
        self.marks = list(marks)
        # The id of each argument's parameter set (for indirect parameters).
        self._ids = ids or {}

    def setmulti(
        self,
        *,
        argnames: Sequence[str],
        valset: Sequence[Any],
        id: Any,
        marks: Iterable[Any],
        scope: Scope,
        param_index: int,
        nodeid: str,
    ) -> CallSpec2:
        params = dict(self.params)
        indices = dict(self.indices)
        arg2scope = dict(self._arg2scope)
        ids = dict(self._ids)
        for arg, value in zip(argnames, valset, strict=True):
            if arg in params:
                raise COLLECT_ERROR(f"{nodeid}: duplicate parametrization of {arg!r}")
            params[arg] = value
            indices[arg] = param_index
            arg2scope[arg] = scope
            ids[arg] = "" if id is HIDDEN_PARAM else id
        return CallSpec2(
            params,
            indices,
            arg2scope,
            self._idlist if id is HIDDEN_PARAM else [*self._idlist, id],
            [*self.marks, *(_normalize_mark(m) for m in marks)],
            ids,
        )

    def getparam(self, name: str) -> Any:
        try:
            return self.params[name]
        except KeyError as exc:
            raise ValueError(name) from exc

    @property
    def id(self) -> str:
        return "-".join(self._idlist)


class FixtureDef:
    """A fixture as registered: where (``node``), and the model's
    ``ResourceDef`` it becomes."""

    __slots__ = ("argnames", "autouse", "ids", "name", "node", "params", "resource", "scope")

    def __init__(self, node: NodeBase | None, resource: ResourceDef, ids: Any = None) -> None:
        self.node = node
        self.resource = resource
        self.name = resource.name
        self.scope = resource.scope
        self.params = resource.params
        self.ids = ids
        self.argnames = resource.argnames
        self.autouse = resource.autouse

    @property
    def scope_name(self) -> str:
        return self.scope.name.lower()

    @property
    def baseid(self) -> str:
        return self.resource.baseid


class DirectParamDef(FixtureDef):
    """What a direct parameter stands for in a closure: no dependencies."""

    def __init__(self, name: str, scope: Scope) -> None:
        super().__init__(None, ResourceDef(name, lambda: None, scope))


class FuncFixtureInfo:
    __slots__ = ("argnames", "initialnames", "name2fixturedefs", "names_closure")

    def __init__(
        self,
        argnames: tuple[str, ...],
        initialnames: tuple[str, ...],
        names_closure: list[str],
        name2fixturedefs: dict[str, Sequence[FixtureDef]],
    ) -> None:
        self.argnames = argnames
        self.initialnames = initialnames
        self.names_closure = names_closure
        self.name2fixturedefs = name2fixturedefs

    def prune_dependency_tree(self) -> None:
        """The closure without what direct parameters made unreachable."""
        closure = set(traverse_fixture_closure(self.initialnames, self.name2fixturedefs.get))
        self.names_closure[:] = [name for name in self.names_closure if name in closure]


class Function(NodeBase, pytest.Function):
    """A test: pytest's item, which conftest hooks see. Its
    ``user_properties`` and ``funcargs`` are made when first asked for, its
    class and instance found then."""

    nofuncargs = False
    _aye_user_properties: list[tuple[str, object]] | None = None
    _aye_funcargs: Any = NOTSET
    _cls: Any = NOTSET
    _instance: Any = NOTSET

    def __init__(  # noqa: PLR0913 - pytest's item takes these
        self,
        frontend: Frontend,
        name: str,
        parent: NodeBase,
        *,
        raw: Any,
        callspec: CallSpec2 | None = None,
        fixtureinfo: FuncFixtureInfo | None = None,
        keywords: dict[str, Any] | None = None,
        originalname: str | None = None,
        directness: dict[str, str] | None = None,
        marks: list[Any] | None = None,
    ) -> None:
        """``marks``: the function's own (``get_unpacked_marks``), when the
        caller read them already for the items it makes of it."""
        self._init_node(frontend, name, parent, None)
        self.nodeid = f"{parent.nodeid}::{name}"  # type: ignore[attr-defined]
        self.aye_id = f"{parent.aye_id}::{name}"  # type: ignore[attr-defined]
        self.originalname = originalname or name
        # The test function as its class or module holds it (a function, a
        # staticmethod or a classmethod).
        self._raw = raw
        self._obj: Any = NOTSET
        self._directness = directness or _NO_DIRECTNESS
        self._case: Case | None = None
        holder = self._marks_holder()
        self.own_markers.extend(get_unpacked_marks(holder) if marks is None else marks)
        if callspec is not None:
            self.callspec = callspec
            self.own_markers.extend(callspec.marks)
        own = self.keywords._markers
        own.update((mark.name, mark) for mark in self.own_markers)
        own.update(safe_getattr(holder, "__dict__", {}))
        if keywords:
            own.update(keywords)
        if fixtureinfo is None:
            fixtureinfo = frontend.getfixtureinfo(self, self._signature_holder(), self.cls)
        self._fixtureinfo = fixtureinfo
        self.fixturenames = fixtureinfo.names_closure

    def _marks_holder(self) -> Any:
        """What pytest reads the marks and keywords from: the function."""
        return _function_of(self._raw)

    def _signature_holder(self) -> Any:
        return self._marks_holder()

    @property
    def module(self) -> Any:
        node = self.getparent(Module)
        return node.obj if node is not None else None

    @property
    def user_properties(self) -> list[tuple[str, object]]:
        found = self._aye_user_properties
        if found is None:
            found = self._aye_user_properties = []
        return found

    @user_properties.setter
    def user_properties(self, value: list[tuple[str, object]]) -> None:
        self._aye_user_properties = value

    @property
    def funcargs(self) -> Any:
        found = self._aye_funcargs
        if found is NOTSET:
            found = self._aye_funcargs = {}
        return found

    @funcargs.setter
    def funcargs(self, value: Any) -> None:
        self._aye_funcargs = value

    @property
    def cls(self) -> type | None:
        found = self._cls
        if found is NOTSET:
            node = self.getparent(Class)
            found = self._cls = node.obj if node is not None else None
        return found

    @property
    def instance(self) -> Any:
        if self._instance is NOTSET:
            parent = self.parent
            self._instance = parent.newinstance() if isinstance(parent, Class) else None
        return self._instance

    @property
    def obj(self) -> Any:
        if self._obj is NOTSET:
            instance = self.instance
            holder = instance if instance is not None else self.parent.obj  # type: ignore[union-attr]
            self._obj = getattr(holder, self.originalname)
        return self._obj

    @obj.setter
    def obj(self, value: Any) -> None:
        self._obj = value

    @property
    def function(self) -> Any:
        return getattr(self.obj, "__func__", self.obj)

    @property
    def location(self) -> tuple[str, int | None, str]:
        path, line = self._frontend.location_of(self)
        return self._frontend.nodeid_of_path(path), line - 1 if line else None, self.getmodpath()

    def reportinfo(self) -> tuple[Any, int | None, str]:
        path, line = self._frontend.location_of(self)
        return path, line - 1 if line else None, self.getmodpath()

    def getmodpath(self, stopatmodule: bool = True, includemodule: bool = False) -> str:
        parts = []
        for node in self.iter_parents():
            if isinstance(node, Module):
                if includemodule:
                    parts.append(os.path.splitext(node.name)[0])
                break
            parts.append(node.name)
        return ".".join(reversed(parts))

    def add_report_section(self, when: str, key: str, content: str) -> None:
        pass

    def runtest(self) -> None:
        raise RuntimeError("aye's engine runs the tests: an item can't run itself")

    setup = runtest


def _function_of(raw: Any) -> Any:
    """A test function as its class or module holds it, unwrapped from a
    ``staticmethod`` or ``classmethod``."""
    if isinstance(raw, staticmethod | classmethod):
        return raw.__func__
    return raw


class TestCaseFunction(Function):
    """A unittest ``TestCase`` method: no fixture arguments, no
    parametrization."""

    __test__ = False
    nofuncargs = True

    @property
    def instance(self) -> Any:
        if self._instance is NOTSET:
            self._instance = self.parent.obj(self.name)  # type: ignore[union-attr]
        return self._instance


class FunctionDefinition(Function, pytest.FunctionDefinition):
    """``metafunc.definition``: the test function before parametrization."""

    def runtest(self) -> None:
        raise RuntimeError("function definitions are not supposed to be run as tests")

    setup = runtest


# ------------------------------------------------------------------ fixtures


def traverse_fixture_closure(
    initialnames: Iterable[str], getfixturedefs: Callable[[str], Sequence[FixtureDef] | None]
) -> Iterator[str]:
    """The names a test needs, depth first from ``initialnames``, each once:
    a fixture's dependencies follow it, and a fixture that requests its own
    name descends into the definition it overrides."""
    current: dict[str, int] = {}

    def visit(argname: str) -> Iterator[str]:
        index = current.get(argname)
        if index == -1:
            return
        if index is None:
            yield argname
            current[argname] = -1
        fixturedefs = getfixturedefs(argname)
        if not fixturedefs:
            return
        index = current.get(argname, -1)
        if -index > len(fixturedefs):
            return
        fixturedef = fixturedefs[index]
        current[argname] = index - 1
        for dependency in fixturedef.argnames:
            yield from visit(dependency)
        current[argname] = index

    for name in initialnames:
        yield from visit(name)


class Registry:
    """Every fixture registered, by name, general to specific (pytest's
    ``_arg2fixturedefs``), and the autouse names of each node."""

    def __init__(self) -> None:
        self.defs: dict[str, list[FixtureDef]] = {}
        self.autouse: dict[int, list[str]] = {}

    def add(self, fixturedef: FixtureDef, node: NodeBase) -> None:
        defs = self.defs.setdefault(fixturedef.name, [])
        for index, existing in enumerate(defs):
            if _more_specific(existing.node, node):
                defs.insert(index, fixturedef)
                break
        else:
            defs.append(fixturedef)
        if fixturedef.autouse:
            self.autouse.setdefault(id(node), []).append(fixturedef.name)

    def visible(self, name: str, node: NodeBase) -> tuple[FixtureDef, ...]:
        """The definitions of ``name`` visible from ``node``, general to
        specific."""
        defs = self.defs.get(name)
        if not defs:
            return ()
        chain = {id(n) for n in node.iter_parents()}
        return tuple(fd for fd in defs if id(fd.node) in chain)


def _more_specific(candidate: NodeBase | None, other: NodeBase | None) -> bool:
    """Whether ``candidate`` lies strictly below ``other``."""
    if candidate is None or other is None or candidate is other:
        return False
    return any(node is other for node in candidate.iter_parents())


# ------------------------------------------------------------------ parametrize


class ParamSet(NamedTuple):
    values: Sequence[Any]
    marks: Sequence[Any]
    id: Any


def _parse_parametrize_args(argnames: Any, *args: Any, **kwargs: Any) -> tuple[list[str], bool]:
    if isinstance(argnames, str):
        trailing = argnames.rstrip().endswith(",")
        names = [x.strip() for x in argnames.split(",") if x.strip()]
        return names, len(names) == 1 and not trailing
    return list(argnames), False


def _parametrize_argnames(mark: Any) -> list[str]:
    return _parse_parametrize_args(*mark.args, **mark.kwargs)[0]


def _extract(value: Any, force_tuple: bool) -> ParamSet:
    if isinstance(value, pytest.ParameterSet):
        return ParamSet(tuple(value.values), [_normalize_mark(m) for m in value.marks], value.id)
    if force_tuple:
        return ParamSet((value,), [], None)
    return ParamSet(value, [], None)


def _resolve_args_directness(argnames: Sequence[str], indirect: Any, nodeid: str) -> dict[str, str]:
    if isinstance(indirect, bool):
        return dict.fromkeys(argnames, "indirect" if indirect else "direct")
    if isinstance(indirect, Sequence):
        directness = dict.fromkeys(argnames, "direct")
        for arg in indirect:
            if arg not in argnames:
                pytest.fail(f"In {nodeid}: indirect fixture '{arg}' doesn't exist", pytrace=False)
            directness[arg] = "indirect"
        return directness
    pytest.fail(
        f"In {nodeid}: expected Sequence or boolean for indirect, got {type(indirect).__name__}",
        pytrace=False,
    )
    raise AssertionError  # unreachable: fail raises


class IdMaker:
    """pytest's parameter set ids: explicit ids, ``ids`` lists and
    callables, ``pytest_make_parametrize_id``, ids from values, then
    ``argname`` and index; made unique with suffixes."""

    def __init__(
        self,
        argnames: Sequence[str],
        parametersets: Sequence[ParamSet],
        idfn: Callable[[Any], Any] | None,
        ids: Sequence[Any] | None,
        config: Config | None,
        nodeid: str | None,
        frontend: Frontend | None = None,
    ) -> None:
        self.argnames = argnames
        self.parametersets = parametersets
        self.idfn = idfn
        self.ids = ids
        self.config = config
        self.nodeid = nodeid
        self.frontend = frontend

    def make_unique_parameterset_ids(self) -> list[Any]:
        resolved = list(self._resolve_ids())
        if len(resolved) == len(set(resolved)):
            return resolved
        counts = Counter(resolved)
        if self._strict():
            shown = ", ".join("<hidden>" if i is HIDDEN_PARAM else i for i in resolved)
            raise COLLECT_ERROR(
                "Duplicate parametrization IDs detected, but strict_parametrization_ids is set."
                f"\n\nTest name:      {self.nodeid}\nIDs:            {shown}"
            )
        suffixes: dict[Any, int] = defaultdict(int)
        originals = set(resolved)
        for index, id_ in enumerate(resolved):
            if counts[id_] > 1:
                if id_ is HIDDEN_PARAM:
                    pytest.fail(
                        f"{self._prefix()}multiple instances of HIDDEN_PARAM cannot be used in "
                        "the same parametrize call, because the tests names need to be unique."
                    )
                suffix = "_" if id_ and id_[-1].isdigit() else ""
                new = f"{id_}{suffix}{suffixes[id_]}"
                while new in originals:
                    suffixes[id_] += 1
                    new = f"{id_}{suffix}{suffixes[id_]}"
                resolved[index] = new
                suffixes[id_] += 1
        return resolved

    def _strict(self) -> bool:
        if self.config is None:
            return False
        value = self.config.getini("strict_parametrization_ids")
        if value is None:
            value = self.config.getini("strict")
        return bool(value)

    def _prefix(self) -> str:
        return f"In {self.nodeid}: " if self.nodeid is not None else ""

    def _resolve_ids(self) -> Iterator[Any]:
        for index, parameterset in enumerate(self.parametersets):
            if parameterset.id is not None:
                if parameterset.id is HIDDEN_PARAM:
                    yield HIDDEN_PARAM
                else:
                    yield self._escaped(parameterset.id)
            elif self.ids and index < len(self.ids) and self.ids[index] is not None:
                if self.ids[index] is HIDDEN_PARAM:
                    yield HIDDEN_PARAM
                else:
                    yield self._required(self.ids[index], index)
            else:
                yield "-".join(
                    self._idval(value, argname, index)
                    for value, argname in zip(parameterset.values, self.argnames, strict=True)
                )

    def _escaped(self, value: str | bytes) -> str:
        if self.config is not None and self.config.getini(
            "disable_test_id_escaping_and_forfeit_all_rights_to_community_support"
        ):
            return value if isinstance(value, str) else value.decode("ascii", "backslashreplace")
        return ascii_escaped(value)

    def _idval(self, value: Any, argname: str, index: int) -> str:
        if self.idfn is not None:
            try:
                made = self.idfn(value)
            except Exception as exc:
                prefix = f"{self.nodeid}: " if self.nodeid is not None else ""
                raise ValueError(
                    f"{prefix}error raised while trying to determine id of parameter "
                    f"'{argname}' at position {index}"
                ) from exc
            if made is not None:
                text = self._from_value(made)
                if text is not None:
                    return text
        if self.frontend is not None and self.config is not None:
            hooked = self.frontend.call_hook(
                "pytest_make_parametrize_id", config=self.config, val=value, argname=argname
            )
            if hooked is not None:
                return hooked
        text = self._from_value(value)
        if text is not None:
            return text
        return f"{argname}{index}"

    def _from_value(self, value: Any) -> str | None:  # noqa: PLR0911 - one return per type
        if isinstance(value, str | bytes):
            return self._escaped(value)
        if value is None or isinstance(value, float | int | bool | complex):
            return str(value)
        if isinstance(value, re.Pattern):
            return ascii_escaped(value.pattern)
        if value is NOTSET:
            return None
        if isinstance(value, enum.Enum):
            return str(value)
        name = safe_getattr(value, "__name__", None)
        if isinstance(name, str):
            return name
        return None

    def _required(self, value: Any, index: int) -> str:
        text = self._from_value(value)
        if text is not None:
            return text
        pytest.fail(
            f"{self._prefix()}ids contains unsupported value {value!r} (type: {type(value)!r}) "
            f"at index {index}. Supported types are: str, bytes, int, float, complex, bool, "
            "enum, regex or anything with a __name__.",
            pytrace=False,
        )
        raise AssertionError  # unreachable: fail raises


class Metafunc(pytest.Metafunc):
    """What ``pytest_generate_tests`` receives."""

    def __init__(
        self,
        frontend: Frontend,
        definition: FunctionDefinition,
        fixtureinfo: FuncFixtureInfo,
        cls: type | None,
        module: Any,
    ) -> None:
        self._frontend = frontend
        self.definition = definition
        self.config = frontend.config
        self.module = module
        self.function = definition.obj
        self.fixturenames = fixtureinfo.names_closure
        self.cls = cls
        self._arg2fixturedefs = fixtureinfo.name2fixturedefs
        self._calls: list[CallSpec2] = []
        self._params_directness: dict[str, str] = {}

    def parametrize(
        self,
        argnames: str | Sequence[str],
        argvalues: Iterable[Any],
        indirect: bool | Sequence[str] = False,
        ids: Any = None,
        scope: str | None = None,
        *,
        _param_mark: Any = None,
    ) -> None:
        """Add calls of the test function, one per parameter set, times the
        calls already there (the first ``parametrize`` varies slowest)."""
        nodeid = self.definition.nodeid
        names, parametersets = self._frontend.parametersets(
            argnames, argvalues, self.function, nodeid
        )
        if "request" in names:
            pytest.fail(
                f"{nodeid}: 'request' is a reserved name and cannot be used in "
                "@pytest.mark.parametrize",
                pytrace=False,
            )
        if scope is not None:
            scope_ = _user_scope(scope, f"parametrize() call in {self.function.__name__}")
        else:
            scope_ = self._find_scope(names, indirect)
        self._validate_used(names, indirect)
        source = getattr(_param_mark, "_param_ids_from", None) if _param_mark is not None else None
        generated = getattr(source, "_param_ids_generated", None) if source is not None else None
        if generated is not None:
            ids = generated
        resolved = self._resolve_ids(names, ids, parametersets)
        if source is not None and generated is None:
            object.__setattr__(source, "_param_ids_generated", resolved)
        directness = _resolve_args_directness(names, indirect, nodeid)
        self._params_directness.update(directness)
        for name in names:
            if directness[name] == "direct":
                self._arg2fixturedefs[name] = [DirectParamDef(name, scope_)]
        calls = []
        for callspec in self._calls or [CallSpec2()]:
            for index, (param_id, paramset) in enumerate(zip(resolved, parametersets, strict=True)):
                calls.append(
                    callspec.setmulti(
                        argnames=names,
                        valset=paramset.values,
                        id=param_id,
                        marks=paramset.marks,
                        scope=scope_,
                        param_index=index,
                        nodeid=nodeid,
                    )
                )
        self._calls = calls

    def _find_scope(self, names: Sequence[str], indirect: Any) -> Scope:
        if isinstance(indirect, Sequence):
            all_fixtures = len(indirect) == len(names)
        else:
            all_fixtures = bool(indirect)
        if all_fixtures:
            scopes = [
                defs[-1].scope
                for name, defs in (self._arg2fixturedefs or {}).items()
                if name in names and defs
            ]
            return max(scopes, default=Scope.FUNCTION)
        return Scope.FUNCTION

    def _validate_used(self, names: Sequence[str], indirect: Any) -> None:
        defaults = set(get_default_arg_names(self.function))
        nodeid = self.definition.nodeid
        for arg in names:
            if arg in self.fixturenames:
                continue
            if arg in defaults:
                pytest.fail(
                    f"In {nodeid}: function already takes an argument '{arg}' with a default value",
                    pytrace=False,
                )
            if isinstance(indirect, Sequence):
                kind = "fixture" if arg in indirect else "argument"
            else:
                kind = "fixture" if indirect else "argument"
            pytest.fail(f"In {nodeid}: function uses no {kind} '{arg}'", pytrace=False)

    def _resolve_ids(
        self, names: Sequence[str], ids: Any, parametersets: list[ParamSet]
    ) -> list[Any]:
        if ids is None:
            idfn, given = None, None
        elif callable(ids):
            idfn, given = ids, None
        else:
            idfn, given = None, self._validate_ids(ids, parametersets)
        return IdMaker(
            names,
            parametersets,
            idfn,
            given,
            self.config,
            self.definition.nodeid,
            self._frontend,
        ).make_unique_parameterset_ids()

    def _validate_ids(self, ids: Any, parametersets: list[ParamSet]) -> list[Any]:
        try:
            count = len(ids)
        except TypeError:
            try:
                iter(ids)
            except TypeError as exc:
                raise TypeError("ids must be a callable or an iterable") from exc
            count = len(parametersets)
        if count not in (len(parametersets), 0):
            pytest.fail(
                f"In {self.definition.nodeid}: {len(parametersets)} parameter sets specified, "
                f"with different number of ids: {count}",
                pytrace=False,
            )
        return list(itertools.islice(ids, count))

    def _recompute_direct_params_indices(self) -> None:
        for argname, kind in self._params_directness.items():
            if kind == "direct":
                for index, callspec in enumerate(self._calls):
                    callspec.indices[argname] = index


def _user_scope(name: str, description: str) -> Scope:
    try:
        return Scope.named(name)
    except KeyError:
        pytest.fail(
            f"{description} got an unexpected scope value '{name}'",
            pytrace=False,
        )
        raise AssertionError from None  # unreachable: fail raises


class _PythonPlugin:
    """pytest's python plugin: ``parametrize`` marks, closest first."""

    def pytest_generate_tests(self, metafunc: Metafunc) -> None:
        for marker in metafunc.definition.iter_markers(name="parametrize"):
            metafunc.parametrize(*marker.args, **marker.kwargs, _param_mark=marker)


class _FixtureManagerPlugin:
    """pytest's ``funcmanage``: fixture ``params``, and ``reorder_items``."""

    def pytest_generate_tests(self, metafunc: Metafunc) -> None:
        marks = list(metafunc.definition.iter_markers("parametrize"))
        for argname in metafunc.fixturenames:
            defs = metafunc._arg2fixturedefs.get(argname, ())
            if any(argname in _parametrize_argnames(mark) for mark in marks):
                continue
            for fixturedef in reversed(defs):
                if fixturedef.params is not None:
                    metafunc.parametrize(
                        argname,
                        fixturedef.params,
                        indirect=True,
                        scope=fixturedef.scope_name,
                        ids=fixturedef.ids,
                    )
                    break
                if argname not in fixturedef.argnames:
                    break

    def pytest_collection_modifyitems(self, items: list[Function]) -> None:
        items[:] = reorder_items(items)


# ------------------------------------------------------------------ reorder


class ParamArgKey(NamedTuple):
    argname: str
    param_index: int
    scoped_item_path: Any
    item_cls: Any


def _param_argkeys(item: Function) -> Iterator[tuple[Scope, ParamArgKey]]:
    """The item's parameters of a scope above function, each with its
    scope, as pytest keys them for ``reorder_items``."""
    callspec = getattr(item, "callspec", None)
    if callspec is None:
        return
    path: str | None = None
    for argname, index in callspec.indices.items():
        scope = callspec._arg2scope[argname]
        if scope is Scope.FUNCTION:
            continue
        if path is None:
            path = os.fspath(item.module_path)
        item_cls = None
        if scope is Scope.SESSION:
            scoped: Any = None
        elif scope is Scope.PACKAGE:
            scoped = os.path.dirname(path)
        elif scope is Scope.MODULE:
            scoped = path
        else:
            scoped = path
            item_cls = item.cls
        yield scope, ParamArgKey(argname, index, scoped, item_cls)


def reorder_items(items: Sequence[Function]) -> list[Function]:
    """pytest's ``reorder_items``: tests that share a parameter of a scope
    above function run together, so that it is set up once per value."""
    argkeys_by_item: dict[Scope, dict[int, dict[ParamArgKey, None]]] = {
        scope: {} for scope in HIGH_SCOPES
    }
    items_by_argkey: dict[Scope, dict[ParamArgKey, OrderedDict[int, None]]] = {
        scope: defaultdict(OrderedDict) for scope in HIGH_SCOPES
    }
    by_id = {id(item): item for item in items}
    for item in items:
        for scope, key in _param_argkeys(item):
            argkeys_by_item[scope].setdefault(id(item), {})[key] = None
            items_by_argkey[scope][key][id(item)] = None
    ordered = _reorder_atscope(
        dict.fromkeys(id(item) for item in items), argkeys_by_item, items_by_argkey, 0
    )
    return [by_id[key] for key in ordered]


def _reorder_atscope(
    items: dict[int, None],
    argkeys_by_item: dict[Scope, dict[int, dict[ParamArgKey, None]]],
    items_by_argkey: dict[Scope, dict[ParamArgKey, OrderedDict[int, None]]],
    level: int,
) -> dict[int, None]:
    # A scope no item has a parameter of keeps the order: the next one's.
    while level < len(HIGH_SCOPES) and not argkeys_by_item[HIGH_SCOPES[level]]:
        level += 1
    if level >= len(HIGH_SCOPES) or len(items) < 3:  # noqa: PLR2004 - pytest's threshold
        return items
    scope = HIGH_SCOPES[level]
    scoped_items = items_by_argkey[scope]
    scoped_keys = argkeys_by_item[scope]
    ignore: set[ParamArgKey] = set()
    queue = deque(items)
    done: dict[int, None] = {}
    while queue:
        no_key: dict[int, None] = {}
        slicing = None
        while queue:
            item = queue.popleft()
            if item in done or item in no_key:
                continue
            keys = dict.fromkeys(k for k in scoped_keys.get(item, ()) if k not in ignore)
            if not keys:
                no_key[item] = None
                continue
            slicing, _ = keys.popitem()
            matching = [i for i in scoped_items[slicing] if i in items]
            for i in reversed(matching):
                queue.appendleft(i)
                for other in HIGH_SCOPES:
                    for key in argkeys_by_item[other].get(i, ()):
                        order = items_by_argkey[other][key]
                        order[i] = None
                        order.move_to_end(i, last=False)
            break
        if no_key:
            done.update(_reorder_atscope(no_key, argkeys_by_item, items_by_argkey, level + 1))
        if slicing is not None:
            ignore.add(slicing)
    return done


# ------------------------------------------------------------------ conftests


class Conftest:
    """A conftest module that loaded."""

    __slots__ = ("directory", "initial", "module", "path", "rel")

    def __init__(self, path: str, module: Any, initial: bool, rel: str) -> None:
        self.path = path
        self.directory = os.path.dirname(path)
        self.module = module
        self.initial = initial
        self.rel = rel


# ------------------------------------------------------------------ the front-end


class Frontend:
    """Reads a pytest suite into aye's model. ``collect`` returns the cases
    of the files it is given, in pytest's order, and the diagnostics; the
    ``policy``, ``hooks``, ``config`` and ``builtins`` go to the engine."""

    def __init__(
        self,
        root: str,
        context: dict[str, Any] | None,
        *,
        builtins: Callable[[Config], dict[str, ResourceDef]] | dict[str, ResourceDef] | None = None,
        invocation_dir: str | None = None,
    ) -> None:
        self.root = os.path.normpath(os.path.abspath(root))
        self.context = context if isinstance(context, dict) else {}
        self.invocation_dir = os.path.abspath(invocation_dir or os.getcwd())
        self.settings = Settings(self.root, self.context)
        self.config = Config(self)
        self.pm = self.config.pluginmanager
        self.parser = Parser(self)
        self.session = Session(self)
        self.registry = Registry()
        self.policy = Policy()
        self.hooks = Hooks()
        self.builtins: dict[str, ResourceDef] = {}
        self._builtins_source = builtins
        self.diagnostics: list[dict[str, Any]] = []
        # Collection outcomes the glue reads: files and directories that
        # failed or were skipped (relative to aye's root), ignored files
        # (absolute: the path ignored), directories whose conftest aye can't
        # load (absolute).
        self.failed_paths: list[str] = []
        self.skipped_paths: list[tuple[str, str | None]] = []
        self.ignored: dict[str, str] = {}
        self.broken: list[str] = []
        # Module names aye's rewriting rewrites (``register_assert_rewrite``,
        # ``pytest_plugins``): the list the rewriting finder reads.
        self.rewrite_modules: list[str] = getattr(pytest, "_aye_rewrite_modules", [])
        self.initial_paths: list[str] = []
        self.deselected: list[Function] = []
        self._configured = False
        self._started = False
        self._dir_nodes: dict[str, DirectoryBase] = {}
        self._dir_state: dict[str, bool] = {}
        self._dirs_loaded: set[str] = set()
        self._conftests: dict[str, Conftest] = {}
        self._failed_conftests: dict[str, _ConftestFailed] = {}
        self._pending: dict[str, Conftest] = {}
        self._models: dict[int, aye_model.Node] = {}
        self._visible: dict[int, dict[str, list[ResourceDef]]] = {}
        self.option_dests: dict[str, str] = {}
        # The options no builtin, conftest or plugin defines, and how many
        # options the parser knew when it found them.
        self._unknown_options: list[str] = []
        self._options_parsed = 0
        self._autoload: list[Any] | None = None
        self._confcutdir: str | None = None
        self._import_mode = "prepend"
        self._blocked_fixtures: set[str] = set()
        self.markeval: dict[str, Any] = {}
        # The builtin plugins ``-p no:NAME`` turns off (the engine leaves out
        # their behaviour: ``logging``, ``warnings``).
        self.disabled: set[str] = set()
        # The logging plugin's live-logging and log-file handlers.
        self._log_handlers = (logging.NullHandler(), logging.NullHandler())
        # Each model node's front-end node, for ``visible``.
        self._owners: dict[int, NodeBase] = {}
        # A file's ``import:`` key when the import-mismatch rule groups it.
        self._grouping_keys: dict[str, str | None] = {}
        # Called when a node's marks change (``add_marker``): the engine
        # running the cases reads them again (``Engine.tags_changed``).
        self.marks_changed: Callable[[], Any] = lambda: None
        self._teardown_impls = False
        # Once per object: a function's location, a mark's tag.
        self._locations: dict[int, tuple[Any, tuple[str, int]]] = {}
        self._mark_tags: dict[int, tuple[Any, Tag]] = {}
        # What the files listed so far reported, by file (as ``extend``
        # was given it).
        self.file_diagnostics: dict[str, list[dict[str, Any]]] = {}
        self._configure_error: _Diagnostic | None = None

    # ------------------------------------------------------------ paths

    def relative(self, path: str | os.PathLike[str]) -> str:
        """``path`` relative to aye's root, with ``/`` separators."""
        absolute = os.path.normpath(os.path.join(self.root, os.fspath(path)))
        if absolute == self.root:
            return "."
        prefix = self.root.rstrip(os.sep) + os.sep
        if absolute.startswith(prefix):
            return absolute[len(prefix) :].replace(os.sep, "/")
        return os.path.relpath(absolute, self.root).replace(os.sep, "/")

    def nodeid_of_path(self, path: str | os.PathLike[str]) -> str:
        """pytest's node id of a path: relative to the rootdir (``.`` for
        it), else to aye's root."""
        absolute = os.path.normpath(os.fspath(path))
        rootdir = self.settings.rootdir
        if absolute == rootdir:
            return "."
        prefix = rootdir.rstrip(os.sep) + os.sep
        if absolute.startswith(prefix):
            return absolute[len(prefix) :].replace(os.sep, "/")
        return self.relative(absolute)

    def absolute(self, path: str) -> str:
        return os.path.normpath(os.path.join(self.root, path))

    def is_initial(self, path: str, *, with_parents: bool = False) -> bool:
        path = os.path.normpath(os.path.abspath(path))
        if path in self.initial_paths:
            return True
        if with_parents:
            prefix = path.rstrip(os.sep) + os.sep
            return any(initial.startswith(prefix) for initial in self.initial_paths)
        return False

    def location_of(self, node: NodeBase) -> tuple[str, int]:
        if isinstance(node, Function):
            filename, line = getfslineno(node._marks_holder())
            return filename or os.fspath(node.module_path), line
        obj = getattr(node, "obj", None)
        if isinstance(node, Class) and obj is not None:
            try:
                return inspect.getsourcefile(obj) or "", inspect.findsource(obj)[1] + 1
            except (TypeError, OSError):
                pass
        return os.fspath(node.path) if node.path is not None else "", 0

    def option_dest(self, name: str) -> str:
        return self.option_dests.get(name, name)

    # ------------------------------------------------------------ diagnostics

    def diagnostic(self, severity: str, code: str, path: str, message: str) -> None:
        self.diagnostics.append(
            {"severity": severity, "code": code, "path": path, "message": message}
        )

    # ------------------------------------------------------------ hooks

    def call_hook(self, name: str, *, directory: str | None = None, **kwargs: Any) -> Any:
        if not self.pm.has(name):
            return None if name in FIRSTRESULT else []
        return self.pm.call(name, kwargs, directory=directory)

    def _hookimpl_opts(self, function: Any) -> dict[str, Any]:
        opts = getattr(function, "_aye_hookimpl", None)
        if isinstance(opts, dict):
            return dict(opts)
        legacy: dict[str, Any] = {}
        marks = {getattr(m, "name", None) for m in safe_getattr(function, "pytestmark", []) or []}
        for option in ("tryfirst", "trylast", "optionalhook", "hookwrapper"):
            if safe_getattr(function, option, False) is True or option in marks:
                legacy[option] = True
        return legacy

    def _hook_impls(self, plugin: Any, where: str) -> tuple[list[HookImpl], list[str]]:
        """The hook implementations of a conftest or plugin, and what aye
        can't run of them (each a message)."""
        impls: list[HookImpl] = []
        problems: list[str] = []
        for name in dir(plugin):
            if not name.startswith("pytest_") or name == "pytest_plugins":
                continue
            function = safe_getattr(plugin, name, None)
            if not inspect.isroutine(function):
                continue
            opts = self._hookimpl_opts(function)
            hook = opts.get("specname") or name
            line = getattr(getattr(function, "__code__", None), "co_firstlineno", 0)
            at = f"{where}:{line}" if line else where
            if hook in DISPLAY_HOOKS:
                continue
            if hook not in HOOKS:
                if opts.get("optionalhook"):
                    continue
                problems.append(
                    f"{at} implements {hook}, which aye's native pytest engine doesn't run "
                    f"(it runs: {', '.join(sorted(HOOKS))}); run the suite with pytest itself to use it"
                )
                continue
            if opts.get("wrapper") or opts.get("hookwrapper"):
                problems.append(
                    f"{at} implements {hook} as a hook wrapper, which aye's native pytest "
                    "engine doesn't run yet; run the suite with pytest itself to use it"
                )
                continue
            impl = HookImpl(hook, function, plugin, opts)
            unknown = [arg for arg in impl.argnames if arg not in HOOKS[hook]]
            if unknown:
                problems.append(
                    f"{at}: {hook} takes {', '.join(unknown)}, which pytest doesn't pass "
                    f"(it passes {', '.join(HOOKS[hook])})"
                )
                continue
            impls.append(impl)
        return impls, problems

    def register_plugin(self, plugin: Any, name: str, where: str) -> None:
        """A plugin that isn't a conftest (``pytest_plugins``, ``-p``, an
        object registered by a conftest): its hooks for every path, its
        fixtures visible everywhere."""
        impls, problems = self._hook_impls(plugin, where)
        if problems:
            for problem in problems:
                self.diagnostic("error", "usage-error", where, problem)
            raise _ConftestFailed(where, None, "unsupported")
        self.pm.add(plugin, name, impls, None)
        self._historic(impls)
        self.parse_fixtures(plugin, self.session)
        if isinstance(plugin, types.ModuleType):
            self.consider_module(plugin, where)

    def _historic(self, impls: list[HookImpl]) -> None:
        """A new plugin's ``pytest_addoption``, and its ``pytest_configure``
        when the session is configured already."""
        for impl in impls:
            if impl.name == "pytest_addoption":
                kwargs = {"parser": self.parser, "pluginmanager": self.pm}
                impl.function(*[kwargs[arg] for arg in impl.argnames])
        if self._configured:
            for impl in impls:
                if impl.name == "pytest_configure":
                    impl.function(*[{"config": self.config}[arg] for arg in impl.argnames])

    def consider_module(self, module: Any, where: str) -> None:
        """A module's ``pytest_plugins``: project modules load as plugins."""
        names = safe_getattr(module, "pytest_plugins", ())
        if isinstance(names, str):
            names = [names]
        for name in names or ():
            self.import_plugin(str(name), where)

    def import_plugin(self, name: str, where: str) -> None:
        """Load the plugin ``name`` (a project module), else report a
        ``usage-error``: aye loads no third-party plugin."""
        if name.startswith("no:"):
            return
        base = name.removeprefix("_pytest.")
        if self.pm.has_plugin(name) or self.pm.is_blocked(name):
            return
        if name.startswith("_pytest.") or base in BUILTIN_PLUGINS:
            if base in ("pytester", "pytester_assertions"):
                self.diagnostic(
                    "error",
                    "usage-error",
                    where,
                    f"{where} loads {name!r}, which runs pytest itself: aye's native engine "
                    "doesn't provide it; run the suite with pytest itself to use it",
                )
                raise _ConftestFailed(where, None, "unsupported")
            return
        self.rewrite_modules.append(name)
        try:
            module = importlib.import_module(name)
        except SKIPPED:
            return
        except Exception as exc:
            self.diagnostic(
                "error",
                "usage-error",
                where,
                f"{where} loads the plugin {name!r}, which failed to import: {_short_traceback(exc)}",
            )
            raise _ConftestFailed(where, exc, "unsupported") from exc
        if not self._is_project(module):
            self.diagnostic(
                "error",
                "usage-error",
                where,
                f"{where} loads the plugin {name!r} ({getattr(module, '__file__', '?')}): aye's "
                "native pytest engine loads no third-party plugin yet; run the suite with pytest "
                "itself to use it",
            )
            raise _ConftestFailed(where, None, "unsupported")
        self.register_plugin(module, name, self.relative(module.__file__))

    def _is_project(self, module: Any) -> bool:
        file = getattr(module, "__file__", None)
        if not file:
            return False
        path = os.path.normpath(os.path.abspath(file))
        if "site-packages" in path.split(os.sep) or "dist-packages" in path.split(os.sep):
            return False
        return any(
            path.startswith(base.rstrip(os.sep) + os.sep)
            for base in (self.root, self.settings.rootdir)
        )

    # ------------------------------------------------------------ configure

    def configure(self) -> None:
        """Settings, options, start-up conftests, ``pytest_configure`` and
        ``pytest_sessionstart``: once (a failure stands for every later
        call)."""
        if self._configure_error is not None:
            raise self._configure_error
        if self._started:
            return
        self._started = True
        try:
            self._configure()
        except _Diagnostic as failure:
            self._configure_error = failure
            raise

    def _configure(self) -> None:
        python = _PythonPlugin()
        self.pm.add(python, "python", self._internal_impls(python), None)
        self._register_builtins()
        self._add_builtin_options()
        self._preparse()
        with self.collection_warnings():
            self._load_initial_conftests()
        self._parse_options()
        self._apply_blocked()
        for plugin in self.config.option.plugins or ():
            self._load_plugin_option(plugin)
        for plugin in os.environ.get("PYTEST_PLUGINS", "").split(","):
            if plugin.strip():
                self._load_plugin_option(plugin.strip())
        self._census()
        self._check_unknown_options()
        self._check_minversion()
        # A plugin a ``pytest_configure`` registers gets its own
        # ``pytest_configure`` at once, as pluggy replays historic hooks.
        self._configured = True
        self._run_global("pytest_configure", config=self.config)
        mark = pytest.mark
        if hasattr(mark, "_config"):
            mark._config = self.config
            mark._markers = set(BUILTIN_MARKERS)
        with self.logging_plugin():
            self._run_global("pytest_sessionstart", session=self.session)
        manager = _FixtureManagerPlugin()
        self.pm.add(manager, "funcmanage", self._internal_impls(manager), None)
        for directory, conftest in list(self._pending.items()):
            if not self._within(directory, self.settings.rootdir):
                del self._pending[directory]
                self.parse_fixtures(conftest.module, self.session)
        self._run_global(
            "pytest_report_header",
            config=self.config,
            start_path=self.config.invocation_params.dir,
            startdir=self.config.invocation_params.dir,
        )
        self.policy = self._policy()

    @staticmethod
    def _internal_impls(plugin: Any) -> list[HookImpl]:
        """The hooks of one of the front-end's own plugins (pytest's
        builtin ones it stands for)."""
        return [
            HookImpl(name, getattr(plugin, name), plugin, {})
            for name in dir(plugin)
            if name.startswith("pytest_")
        ]

    def _run_global(self, name: str, **kwargs: Any) -> list[Any]:
        """A global hook outside any test: a failure is reported (a
        ``UsageError`` as pytest's usage error) and ends the session."""
        try:
            return self.pm.call(name, kwargs)
        except pytest.UsageError as exc:
            raise _Diagnostic("error", "usage-error", ".", f"{name}: {exc}") from exc
        except (*OUTCOMES,) as exc:
            raise _Diagnostic(
                "error",
                "internal-error",
                ".",
                f"{name} raised:\n{_short_traceback(exc)}",
            ) from exc

    def _register_builtins(self) -> None:
        source = self._builtins_source
        engine: dict[str, ResourceDef] = {}
        if callable(source):
            engine = dict(source(self.config))
        elif isinstance(source, dict):
            engine = dict(source)
        own = {
            "pytestconfig": ResourceDef(
                "pytestconfig", lambda: self.config, Scope.SESSION, location="<pytest>"
            ),
            "cache": ResourceDef("cache", lambda: self.config.cache, Scope.SESSION),
        }
        for name, resource in {
            **engine,
            **{k: v for k, v in own.items() if k not in engine},
        }.items():
            self.builtins[name] = resource
            self.registry.add(FixtureDef(self.session, resource), self.session)

    def _add_builtin_options(self) -> None:
        for names, attrs in BUILTIN_OPTIONS:
            option = _Option(names, attrs)
            self.parser.options.append(option)

    def _preparse(self) -> None:
        """What pytest reads before the conftests: ``-o``, ``-p no:``,
        ``--confcutdir``, ``--noconftest``, ``--import-mode``, then
        ``pythonpath``."""
        namespace = self._parse(self.settings.args, conftests=False)
        self.config.set_overrides(namespace.override_ini or ())
        self.config.option = namespace
        self.config.known_args_namespace = namespace
        native = self.settings.native
        mode = native.get("import_mode") or namespace.importmode or "prepend"
        self._import_mode = str(mode)
        confcutdir = native.get("confcutdir") or namespace.confcutdir
        if confcutdir:
            self._confcutdir = os.path.normpath(os.path.join(self.invocation_dir, confcutdir))
        elif self.settings.inifile:
            self._confcutdir = os.path.dirname(self.settings.inifile)
        else:
            self._confcutdir = self.settings.rootdir
        for path in reversed(self.config.getini("pythonpath")):
            sys.path.insert(0, os.fspath(path))
        self.initial_paths = list(self.settings.roots) or self._default_paths()

    def _default_paths(self) -> list[str]:
        """pytest's paths without arguments: ``testpaths`` (globbed) from the
        rootdir, else the invocation directory."""
        if os.path.normpath(self.invocation_dir) == self.settings.rootdir:
            found: list[str] = []
            for pattern in self.config.getini("testpaths"):
                found += sorted(
                    glob.glob(str(pattern), root_dir=self.invocation_dir, recursive=True)
                )
            if found:
                return [os.path.normpath(os.path.join(self.invocation_dir, p)) for p in found]
        return [self.invocation_dir]

    def _parse(self, args: Sequence[str], *, conftests: bool) -> argparse.Namespace:
        parser = argparse.ArgumentParser(add_help=False, allow_abbrev=False, exit_on_error=False)
        self.option_dests = {}
        options = self.parser.options if conftests else self.parser.options[: len(BUILTIN_OPTIONS)]
        for option in options:
            attrs = {k: v for k, v in option.attrs.items() if k not in ("help",)}
            try:
                action = parser.add_argument(*option.names, **attrs)
            except (argparse.ArgumentError, TypeError, ValueError) as exc:
                raise _Diagnostic(
                    "error", "usage-error", ".", f"option {option.names}: {exc}"
                ) from exc
            for name in option.names:
                self.option_dests[name] = action.dest
        parser.add_argument("file_or_dir", nargs="*")
        try:
            namespace, unknown = parser.parse_known_args(list(args))
        except (argparse.ArgumentError, SystemExit) as exc:
            raise _Diagnostic("error", "usage-error", ".", f"pytest's options: {exc}") from exc
        if conftests:
            # Judged once the plugins are loaded (``_check_unknown_options``).
            self._unknown_options = [arg for arg in unknown if arg.startswith("-")]
            self._options_parsed = len(options)
        return namespace

    def _check_unknown_options(self) -> None:
        """Options nothing loaded defines. A ``-p`` or ``PYTEST_PLUGINS``
        plugin may define some (pytest loads those before it parses): the
        options are parsed again with theirs. pytest stops with a usage
        error on what is left, unless it would load a plugin by itself that
        aye doesn't (a ``pytest11`` entry point, whose options aye can't
        know): then the option may be that plugin's, a warning, and aye
        ignores it."""
        if not self._unknown_options:
            return
        if len(self.parser.options) > self._options_parsed:
            self._parse_options()
            if not self._unknown_options:
                return
        unknown = " ".join(self._unknown_options)
        plugins = sorted({entry.name for entry in self._autoloaded()})
        if not plugins:
            raise _Diagnostic(
                "error", "usage-error", ".", f"pytest's options: unrecognized arguments: {unknown}"
            )
        self.diagnostic(
            "warning",
            "unknown-option",
            ".",
            "aye's native pytest engine doesn't know these options and ignores them; a "
            f"plugin pytest would load and aye doesn't ({', '.join(plugins)}) may define "
            f"them: {unknown}",
        )

    def _parse_options(self) -> None:
        namespace = self._parse(self.settings.args, conftests=True)
        self.config.option = namespace
        self.config.known_args_namespace = namespace
        self.config._parsed = True
        self.config.set_overrides(namespace.override_ini or ())
        native_options = self.settings.native.get("options")
        if isinstance(native_options, dict):
            for dest, value in native_options.items():
                if dest not in ("raw", "args") and not hasattr(namespace, dest):
                    setattr(namespace, dest, value)

    def _apply_blocked(self) -> None:
        for plugin in self.config.option.plugins or ():
            if not plugin.startswith("no:"):
                continue
            name = plugin[3:]
            if name in ESSENTIAL_PLUGINS:
                raise _Diagnostic("error", "usage-error", ".", f"plugin {name} cannot be disabled")
            self.pm.set_blocked(name)
            self.disabled.add(name)
            for fixture in PLUGIN_FIXTURES.get(name, ()):
                self._blocked_fixtures.add(fixture)
        for name in self._blocked_fixtures:
            builtin = self.builtins.pop(name, None)
            defs = self.registry.defs.get(name)
            if builtin is not None and defs:
                defs[:] = [d for d in defs if d.resource is not builtin]

    def _autoloaded(self) -> list[Any]:
        """The plugins pytest would load from the environment by itself
        (``pytest11`` entry points, unless ``PYTEST_DISABLE_PLUGIN_AUTOLOAD``
        or ``-p no:NAME``), but those loaded already: aye loads none."""
        if self._autoload is not None:
            return self._autoload
        self._autoload = []
        if os.environ.get("PYTEST_DISABLE_PLUGIN_AUTOLOAD") or getattr(
            self.config.option, "disable_plugin_autoload", False
        ):
            return self._autoload
        try:
            from importlib.metadata import entry_points  # noqa: PLC0415 - only the census needs it

            found = list(entry_points(group="pytest11"))
        except Exception:  # noqa: BLE001 - broken metadata: nothing to report
            return self._autoload
        for entry in found:
            module = entry.value.partition(":")[0]
            if not (self.pm.is_blocked(entry.name) or self.pm.has_plugin(module)):
                self._autoload.append(entry)
        return self._autoload

    def _census(self) -> None:
        """The plugins pytest would load by itself (``_autoloaded``). Those
        whose job aye does are fine; any other is a warning that names it,
        since tests needing its fixtures, marks or hooks won't behave as
        under pytest."""
        for entry in self._autoloaded():
            module = entry.value.partition(":")[0]
            if entry.name in ABSORBED_PLUGINS or module in ABSORBED_PLUGINS:
                continue
            dist = getattr(getattr(entry, "dist", None), "name", None) or module
            self.diagnostic(
                "warning",
                "plugin-not-loaded",
                ".",
                f"pytest would load the plugin {entry.name!r} ({dist}); aye's native pytest "
                "engine loads no third-party plugin yet, so tests that need its fixtures, "
                f"marks or hooks won't run as under pytest (`-p no:{entry.name}` silences "
                "this; run the suite with pytest itself to use it)",
            )

    def _load_plugin_option(self, plugin: str) -> None:
        if plugin.startswith("no:"):
            return
        try:
            self.import_plugin(plugin, "-p")
        except _ConftestFailed:
            raise _Diagnostic("error", "usage-error", ".", f"-p {plugin}: see above") from None

    def _check_minversion(self) -> None:
        minver = self.config.getini("minversion")
        if not minver:
            return
        wanted = tuple(int(x) for x in re.findall(r"\d+", str(minver))[:3])
        if wanted > (9, 1, 1):
            raise _Diagnostic(
                "error",
                "usage-error",
                self.relative(self.settings.inifile or self.root),
                f"{self.settings.inifile}: 'minversion' requires pytest-{minver}, actual "
                "pytest-9.1.1 (aye's native pytest)",
            )

    def _policy(self) -> Policy:
        strict = self.config.getini("strict_xfail")
        if strict is None:
            strict = self.config.getini("strict")
        native = self.settings.native
        if "xfail_strict" in native and native["xfail_strict"] is not None:
            strict = bool(native["xfail_strict"])
        return Policy(
            xfail_strict=bool(strict),
            condition_namespace={"os": os, "sys": sys, "platform": platform, "config": self.config},
            skip_exceptions=(SKIPPED, unittest.SkipTest),
            xfail_exceptions=(XFAILED,),
            fail_exceptions=(FAILED,),
            exit_exceptions=(EXIT,),
        )

    # ------------------------------------------------------------ logging

    @contextmanager
    def logging_plugin(self) -> Iterator[None]:
        """What pytest's logging plugin attaches around
        ``pytest_sessionstart``, collection, the test loop and
        ``pytest_sessionfinish`` (its ``catching_logs``): its live-logging
        and log-file handlers, null ones here (aye writes no log file), on
        the root logger and the loggers that don't propagate, the root's
        level lowered to theirs when one is set (``log_cli_level``,
        ``log_file_level``, else ``log_level``). So a
        ``logging.basicConfig()`` a test module or a test calls does
        nothing, as under pytest. Nothing before ``pytest_configure``, or
        with ``-p no:logging``."""
        if not self._configured or "logging" in self.disabled:
            yield
            return
        with ExitStack() as stack:
            settings = ("log_cli_level", "log_file_level")
            for handler, setting in zip(self._log_handlers, settings, strict=True):
                level = self._log_level(setting, "log_level")
                stack.enter_context(_catching_logs(handler, level))
            yield

    def _log_level(self, *settings: str) -> int | None:
        """The first of ``settings`` set (the option, else the ini value),
        as a level, as pytest's ``get_log_level_for_setting`` reads it."""
        for setting in settings:
            value = self.config.getoption(setting, None)
            if value is None:
                value = self.config.getini(setting)
            if value:
                break
        else:
            return None
        if isinstance(value, str):
            value = value.upper()
        try:
            return int(getattr(logging, str(value), value))
        except (TypeError, ValueError):
            return None

    # ------------------------------------------------------------ warnings

    @contextmanager
    def collection_warnings(self) -> Iterator[None]:
        """pytest's warning filters while it configures and collects: the
        ``filterwarnings`` ini lines, then ``-W``; warnings are recorded,
        not shown, and a filter that makes one an error makes it raise."""
        with warnings.catch_warnings(record=True):
            if not sys.warnoptions:
                warnings.filterwarnings("always", category=DeprecationWarning)
                warnings.filterwarnings("always", category=PendingDeprecationWarning)
            filters = [(line, False) for line in self.config.getini("filterwarnings")]
            filters += [
                (line, True) for line in getattr(self.config.option, "pythonwarnings", None) or ()
            ]
            for line, escape in filters:
                try:
                    parsed = parse_warning_filter(line, escape=escape)
                except (pytest.UsageError, ImportError, AttributeError) as exc:
                    raise _Diagnostic(
                        "error",
                        "usage-error",
                        self.relative(self.settings.inifile or self.root),
                        f"the warning filter {line!r} can't be applied: {exc}",
                    ) from exc
                warnings.filterwarnings(*parsed)
            yield

    # ------------------------------------------------------------ conftests

    def _in_confcutdir(self, path: str) -> bool:
        if self._confcutdir is None:
            return True
        cut = self._confcutdir
        return not (cut != path and cut.startswith(path.rstrip(os.sep) + os.sep))

    @staticmethod
    def _within(path: str, directory: str) -> bool:
        return path == directory or path.startswith(directory.rstrip(os.sep) + os.sep)

    def _load_initial_conftests(self) -> None:
        anchors: list[str] = []
        for path in self.initial_paths:
            if not os.path.exists(path):
                continue
            anchors.append(path)
            if os.path.isdir(path):
                anchors += sorted(
                    os.path.join(path, name)
                    for name in os.listdir(path)
                    if name.startswith("test") and os.path.isdir(os.path.join(path, name))
                )
        if not anchors:
            anchors = [
                self.invocation_dir,
                *sorted(
                    os.path.join(self.invocation_dir, name)
                    for name in os.listdir(self.invocation_dir)
                    if name.startswith("test")
                    and os.path.isdir(os.path.join(self.invocation_dir, name))
                ),
            ]
        for anchor in anchors:
            directory = anchor if os.path.isdir(anchor) else os.path.dirname(anchor)
            try:
                self._load_conftests(directory, initial=True)
            except _ConftestFailed as failure:
                self._broken_at_start(failure)

    def _broken_at_start(self, failure: _ConftestFailed) -> None:
        """A start-up conftest that can't load: pytest's usage error. The
        files below its directory give no tests; the others still do."""
        directory = os.path.dirname(failure.path) if failure.path.endswith(".py") else self.root
        if directory not in self.broken:
            self.broken.append(directory)
        if failure.kind == "unsupported":
            return
        rel = self.relative(failure.path)
        kind = "Skipped" if failure.kind == "skip" else "ImportError"
        self.diagnostic(
            "error",
            "usage-error",
            rel,
            failure.message or self._conftest_message(rel, failure, kind),
        )

    def _conftest_message(self, rel: str, failure: _ConftestFailed, kind: str) -> str:
        cause = failure.cause
        if cause is None:
            return f"{kind} while loading conftest '{rel}'."
        return f"{kind} while loading conftest '{rel}'.\n{_short_traceback(cause)}"

    def _load_conftests(self, directory: str, *, initial: bool) -> None:
        """Import the conftests from the top down to ``directory`` that
        aren't loaded yet (pytest's ``_loadconftestmodules``)."""
        if getattr(self.config.option, "noconftest", False) or directory in self._dirs_loaded:
            return
        chain = [directory]
        while True:
            parent = os.path.dirname(chain[-1])
            if parent == chain[-1]:
                break
            chain.append(parent)
        for parent in reversed(chain):
            if self._in_confcutdir(parent):
                path = os.path.join(parent, "conftest.py")
                if os.path.isfile(path):
                    self._import_conftest(path, initial=initial)
        self._dirs_loaded.add(directory)

    def _import_conftest(self, path: str, *, initial: bool) -> Conftest:
        if path in self._conftests:
            return self._conftests[path]
        if path in self._failed_conftests:
            raise self._failed_conftests[path]
        if resolve_package_path(path) is None:
            sys.modules.pop("conftest", None)
        rel = self.relative(path)
        try:
            module = self.import_path(path)
        except (*OUTCOMES,) as exc:
            kind = "skip" if isinstance(exc, SKIPPED | unittest.SkipTest) else "error"
            failure = _ConftestFailed(path, exc, kind)
            self._failed_conftests[path] = failure
            raise failure from exc
        if hasattr(module, "pytest_plugins") and self._configured:
            message = (
                "Defining 'pytest_plugins' in a non-top-level conftest is no longer supported:\n"
                "It affects the entire test suite instead of just below the conftest as expected.\n"
                f"  {path}\nPlease move it to a top level conftest file at the rootdir:\n"
                f"  {self._confcutdir}"
            )
            failure = _ConftestFailed(path, None, "error", message)
            self._failed_conftests[path] = failure
            raise failure
        conftest = Conftest(path, module, initial, rel)
        impls, problems = self._hook_impls(module, rel)
        if problems:
            for problem in problems:
                self.diagnostic("error", "usage-error", rel, problem)
            failure = _ConftestFailed(path, None, "unsupported")
            self._failed_conftests[path] = failure
            raise failure
        self._conftests[path] = conftest
        self.pm.add(module, path, impls, conftest.directory)
        try:
            self.consider_module(module, rel)
            self._historic(impls)
        except _ConftestFailed as failure:
            failure = _ConftestFailed(path, None, "unsupported")
            self._failed_conftests[path] = failure
            raise failure from None
        except (*OUTCOMES,) as exc:
            failure = _ConftestFailed(path, exc, "error")
            self._failed_conftests[path] = failure
            raise failure from exc
        node = self._dir_nodes.get(conftest.directory)
        if node is not None and self._dir_state.get(conftest.directory):
            self.parse_fixtures(module, node)
        elif self._started and not self._within(conftest.directory, self.settings.rootdir):
            self.parse_fixtures(module, self.session)
        else:
            self._pending[conftest.directory] = conftest
        return conftest

    # ------------------------------------------------------------ imports

    def import_path(self, path: str) -> types.ModuleType:
        """Import the file at ``path`` as pytest's import mode does."""
        if self._import_mode == "importlib":
            return self._import_importlib(path)
        namespace = bool(self.config.getini("consider_namespace_packages"))
        root, name = self._pkg_root_and_name(path, namespace)
        if self._import_mode == "append":
            if root not in sys.path:
                sys.path.append(root)
        elif not sys.path or sys.path[0] != root:
            sys.path.insert(0, root)
        importlib.import_module(name)
        module = sys.modules[name]
        if os.path.basename(path) == "__init__.py":
            return module
        if os.environ.get("PY_IGNORE_IMPORTMISMATCH", "") != "1":
            module_file = getattr(module, "__file__", None)
            if module_file is None:
                raise _ImportMismatch(name, module_file, path)
            if module_file.endswith((".pyc", ".pyo")):
                module_file = module_file[:-1]
            module_file = module_file.removesuffix(os.sep + "__init__.py")
            if not _same_file(path, module_file):
                raise _ImportMismatch(name, module_file, path)
        return module

    def _pkg_root_and_name(self, path: str, namespace: bool) -> tuple[str, str]:
        """The directory to import ``path`` from and its module name: its
        packages', else its own directory and its stem."""
        resolved = _resolve_pkg_root_and_module_name(path, namespace)
        if resolved is not None:
            return resolved
        return os.path.dirname(path), os.path.splitext(os.path.basename(path))[0]

    def _import_importlib(self, path: str) -> types.ModuleType:
        """pytest's ``importlib`` mode: the module's name from its packages
        when that imports it, else a name unique in the rootdir, without
        touching ``sys.path``."""
        namespace = bool(self.config.getini("consider_namespace_packages"))
        resolved = _resolve_pkg_root_and_module_name(path, namespace)
        if resolved is not None:
            name = resolved[1]
            if name in sys.modules:
                return sys.modules[name]
            module = _import_module_using_spec(name, path, insert_modules=False)
            if module is not None:
                return module
        name = module_name_from_path(path, self.settings.rootdir)
        if name in sys.modules:
            return sys.modules[name]
        module = _import_module_using_spec(name, path, insert_modules=True)
        if module is None:
            raise ImportError(f"Can't find module {name} at location {path}")
        return module

    def should_rewrite(self, path: str, name: str | None = None) -> bool:
        """Whether aye's rewriting rewrites the module at ``path``: a
        conftest, a file the user named, a file ``python_files`` matches, or
        a module registered for rewriting (``pytest_plugins``,
        ``register_assert_rewrite``) — pytest's rules."""
        if os.path.basename(path) == "conftest.py":
            return True
        absolute = os.path.normpath(os.path.abspath(path))
        if absolute in self.initial_paths:
            return True
        try:
            patterns = self.config.getini("python_files")
        except ValueError:
            patterns = INI["python_files"][1]
        if any(fnmatch_ex(pattern, absolute) for pattern in patterns):
            return True
        if name is not None:
            return any(name == m or name.startswith(m + ".") for m in self.rewrite_modules)
        return False

    # ------------------------------------------------------------ collect

    def collect(
        self,
        paths: Sequence[str],
        select: Callable[[list[Function]], list[Function]] | None = None,
    ) -> tuple[list[Case], list[dict[str, Any]]]:
        """The cases of the test files ``paths`` (relative to aye's root),
        in pytest's order, and the diagnostics.

        Without ``select`` (a refinement), every item is a case, those the
        ``pytest_collection_modifyitems`` hooks drop included, each after
        the item collected before it: refinement predicts a superset. With
        ``select`` (a batch), it narrows the items before the hooks see
        them, and what the hooks drop is gone (``deselected``)."""
        try:
            self.configure()
            with self.collection_warnings():
                items = self._collect_files(paths)
                kept = select(items) if select is not None else list(items)
                kept = self._modifyitems(kept)
        except _Diagnostic as stop:
            self._stopped(stop)
            return [], self.diagnostics
        with no_automatic_gc():
            if select is None:
                kept = self._with_deselected(items, kept)
            self.session.items = kept
            self.session.testscollected = len(kept)
            cases = self._cases(kept)
            self._fill_visible()
            self.hooks = self._hooks()
        return cases, self.diagnostics

    def extend(self, paths: Sequence[str]) -> list[Case]:
        """List the test files ``paths`` (relative to aye's root) into the
        session, as one more step of its collection: their items, the
        ``pytest_collection_modifyitems`` hooks over them, their cases.
        Returns the new cases in pytest's order, each item the hooks drop
        after the item collected before it (``deselected`` holds those);
        ``session.items`` gains the others. The diagnostics go to
        ``diagnostics``, those raised while a file was collected also to
        ``file_diagnostics[file]``. A file listed before is left out:
        nothing is imported twice."""
        paths = [path for path in dict.fromkeys(paths) if path not in self.file_diagnostics]
        if not paths:
            return []
        for path in paths:
            self.file_diagnostics[path] = []
        try:
            self.configure()
            with self.collection_warnings(), self.logging_plugin():
                items = self._collect_files(paths)
                kept = self._modifyitems(list(items))
        except _Diagnostic as stop:
            self._stopped(stop)
            return []
        with no_automatic_gc():
            listed = self._with_deselected(items, kept)
            self.session.items = [*self.session.items, *kept]
            self.session.testscollected = len(self.session.items)
            cases = self._cases(listed)
            self._fill_visible()
            self.hooks = self._hooks()
        settle_gc()
        return cases

    def _stopped(self, stop: _Diagnostic) -> None:
        """A collection step that couldn't go on: its diagnostic, once."""
        if not any(found is stop.fields for found in self.diagnostics):
            self.diagnostics.append(stop.fields)

    def _collect_files(self, paths: Sequence[str]) -> list[Function]:
        items: list[Function] = []
        self._run_global("pytest_collection", session=self.session)
        for path in dict.fromkeys(paths):
            start = len(self.diagnostics)
            for item in self._collect_file(self.absolute(path)):
                items.append(item)
                if not self.pm.has("pytest_itemcollected"):
                    continue
                try:
                    item.ihook.pytest_itemcollected(item=item)
                except (*OUTCOMES,) as exc:
                    raise _Diagnostic(
                        "error",
                        "internal-error",
                        item.aye_id,
                        f"pytest_itemcollected raised:\n{_short_traceback(exc)}",
                    ) from exc
            self.file_diagnostics.setdefault(path, []).extend(self.diagnostics[start:])
        return items

    def _modifyitems(self, items: list[Function]) -> list[Function]:
        items = list(items)
        self._run_global(
            "pytest_collection_modifyitems", session=self.session, config=self.config, items=items
        )
        self._run_global("pytest_collection_finish", session=self.session)
        try:
            self.markeval = self._markeval()
        except (*OUTCOMES,) as exc:
            raise _Diagnostic(
                "error",
                "internal-error",
                ".",
                f"pytest_markeval_namespace raised:\n{_short_traceback(exc)}",
            ) from exc
        self.policy.condition_namespace.update(self.markeval)
        return items

    def _markeval(self) -> dict[str, Any]:
        namespace: dict[str, Any] = {}
        for result in reversed(self.pm.call("pytest_markeval_namespace", {"config": self.config})):
            if not isinstance(result, dict):
                raise TypeError(
                    f'"pytest_markeval_namespace" needs to return a dict, got {result!r}'
                )
            namespace.update(result)
        return namespace

    def _with_deselected(self, collected: list[Function], kept: list[Function]) -> list[Function]:
        """``kept`` in its order, with each item the hooks dropped placed
        after the item collected before it."""
        kept_ids = {id(item) for item in kept}
        if len(kept_ids) == len(kept) == len(collected) and all(
            id(item) in kept_ids for item in collected
        ):
            return list(kept)  # none dropped, none twice: ``kept`` as it is
        after: dict[int | None, list[Function]] = {}
        previous: int | None = None
        for item in collected:
            if id(item) in kept_ids:
                previous = id(item)
            else:
                after.setdefault(previous, []).append(item)
                self.deselected.append(item)
        ordered = after.pop(None, [])
        for item in kept:
            ordered.append(item)
            ordered += after.pop(id(item), [])
        seen: set[int] = set()
        result = []
        for item in ordered:
            if id(item) not in seen:
                seen.add(id(item))
                result.append(item)
        return result

    def _under_broken(self, path: str) -> bool:
        return any(self._within(path, directory) for directory in self.broken)

    def _collect_file(self, path: str) -> list[Function]:  # noqa: PLR0911 - one return per rule
        rel = self.relative(path)
        if not os.path.isfile(path):
            self.diagnostic("error", "not-found", rel, f"{rel}: no such file")
            return []
        if self._under_broken(path) or not path.endswith(".py"):
            return []
        parent: NodeBase = self.session
        for directory in self._dir_chain(path):
            if parent is not self.session and self._ignored(directory, parent, is_dir=True):
                self.ignored[path] = directory
                self._report_ignored(path, directory)
                return []
            node = self._dir_node(directory, parent)
            if not self._collect_dir(node):
                return []
            parent = node
        if self._ignored(path, parent, is_dir=False):
            self.ignored[path] = path
            self._report_ignored(path, path)
            return []
        if not self.is_initial(path) and not any(
            fnmatch_ex(pattern, path) for pattern in self.config.getini("python_files")
        ):
            return []
        module = Module(self, parent, path)
        return self._collect_module(module)

    def _dir_chain(self, path: str) -> list[str]:
        """The directories pytest collects down to ``path``: from the
        outermost one within the ``confcutdir`` to the file's."""
        chain = []
        directory = os.path.dirname(path)
        while True:
            if chain and not self._in_confcutdir(directory):
                break
            chain.append(directory)
            parent = os.path.dirname(directory)
            if parent == directory:
                break
            directory = parent
        return chain[::-1]

    def _dir_node(self, path: str, parent: NodeBase) -> DirectoryBase:
        node = self._dir_nodes.get(path)
        if node is None:
            if os.path.isfile(os.path.join(path, "__init__.py")):
                node = Package(self, parent, path)
            else:
                node = Dir(self, parent, path)
            self._dir_nodes[path] = node
        return node

    def _collect_dir(self, node: DirectoryBase) -> bool:
        """Load a directory's conftests, as pytest does before collecting
        it: an import error fails it, a skip skips it. Whether its files can
        be collected."""
        path = os.fspath(node.path)
        state = self._dir_state.get(path)
        if state is not None:
            return state
        try:
            self._load_conftests(path, initial=False)
        except _ConftestFailed as failure:
            self._dir_state[path] = False
            self._conftest_collection_failed(node, failure)
            return False
        conftest = self._pending.pop(path, None)
        try:
            if conftest is not None:
                self.parse_fixtures(conftest.module, node)
            if isinstance(node, Package):
                self._register_package_setup(node)
        except (*OUTCOMES,) as exc:
            self._dir_state[path] = False
            rel = self.relative(path)
            self.failed_paths.append(rel)
            self.diagnostic("error", "collection-error", rel, _short_traceback(exc))
            return False
        self._dir_state[path] = True
        return True

    def _conftest_collection_failed(self, node: DirectoryBase, failure: _ConftestFailed) -> None:
        rel_dir = self.relative(os.fspath(node.path))
        if failure.kind == "unsupported":
            self.broken.append(
                os.path.dirname(failure.path)
                if failure.path.endswith(".py")
                else os.fspath(node.path)
            )
            return
        if failure.kind == "skip":
            reason = getattr(failure.cause, "msg", None) or str(failure.cause)
            self.skipped_paths.append((rel_dir, reason))
            self.diagnostic("info", "skipped", rel_dir, reason or "skipped by pytest")
            return
        rel = self.relative(failure.path)
        self.failed_paths.append(rel_dir)
        kind = "Failed" if isinstance(failure.cause, FAILED) else "ImportError"
        message = failure.message or self._conftest_message(rel, failure, kind)
        self.diagnostic("error", "collection-error", rel, message)

    def _ignored(self, path: str, parent: NodeBase, *, is_dir: bool) -> bool:
        """pytest's ignore rules for ``path``, collected by ``parent``;
        never for what the user named (or a directory above it)."""
        if self.is_initial(path, with_parents=is_dir):
            return False
        directory = os.fspath(parent.path) if parent.path is not None else None
        result = self.pm.call(
            "pytest_ignore_collect",
            {"collection_path": Path(path), "path": Path(path), "config": self.config},
            directory=directory,
        )
        if result is not None:
            return bool(result)
        return self._default_ignore(path, is_dir=is_dir)

    def _default_ignore(self, path: str, *, is_dir: bool) -> bool:
        if os.path.basename(path) == "__pycache__":
            return True
        parent = os.path.dirname(path)
        ignore = [os.fspath(p) for p in self._conftest_pathlist("collect_ignore", parent)]
        ignore += [os.path.abspath(x) for x in self.config.getoption("ignore") or ()]
        if path in ignore:
            return True
        globs = [os.fspath(p) for p in self._conftest_pathlist("collect_ignore_glob", parent)]
        globs += [os.path.abspath(x) for x in self.config.getoption("ignore_glob") or ()]
        if any(fnmatch.fnmatch(path, glob) for glob in globs):
            return True
        if not self.config.getoption("collect_in_virtualenv") and _in_venv(path):
            return True
        if is_dir:
            return any(fnmatch_ex(pattern, path) for pattern in self.config.getini("norecursedirs"))
        return False

    def _conftest_pathlist(self, name: str, directory: str) -> list[Path]:
        """The closest conftest's ``collect_ignore`` (or ``_glob``), its
        entries relative to its directory."""
        conftests = sorted(
            (c for c in self._conftests.values() if self._within(directory, c.directory)),
            key=lambda c: len(c.directory),
            reverse=True,
        )
        for conftest in conftests:
            try:
                entries = getattr(conftest.module, name)
            except AttributeError:
                continue
            values = []
            for entry in entries:
                if isinstance(entry, os.PathLike):
                    values.append(Path(entry))
                else:
                    values.append(
                        Path(
                            os.path.abspath(
                                os.path.join(conftest.directory, entry.replace("/", os.sep))
                            )
                        )
                    )
            return values
        return []

    def _report_ignored(self, path: str, ignored: str) -> None:
        file = self.relative(path)
        what = "this file" if ignored == path else f"`{self.relative(ignored)}`"
        rules = "collect_ignore, --ignore, norecursedirs or a pytest_ignore_collect hook"
        self.diagnostic(
            "info",
            "ignored",
            file,
            f"pytest ignores {what} ({rules}), so it collects no tests from this file",
        )

    # ------------------------------------------------------------ modules

    def _import_test_module(self, module: Module) -> Any:
        path = os.fspath(module.path)
        try:
            return self.import_path(path)
        except SyntaxError as exc:
            raise COLLECT_ERROR(_short_traceback(exc)) from exc
        except _ImportMismatch as exc:
            name, module_file, wanted = exc.args
            raise COLLECT_ERROR(
                "import file mismatch:\n"
                f"imported module {name!r} has this __file__ attribute:\n  {module_file}\n"
                f"which is not the same as the test file we want to collect:\n  {wanted}\n"
                "HINT: remove __pycache__ / .pyc files and/or use a unique basename for your "
                "test file modules"
            ) from exc
        except ImportError as exc:
            raise COLLECT_ERROR(
                f"ImportError while importing test module '{path}'.\n"
                "Hint: make sure your test modules/packages have valid Python names.\n"
                f"Traceback:\n{_short_traceback(exc)}"
            ) from exc
        except SKIPPED as exc:
            if getattr(exc, "allow_module_level", False):
                raise _SkipCollect(getattr(exc, "msg", None) or str(exc)) from exc
            raise COLLECT_ERROR(
                "Using pytest.skip outside of a test will skip the entire module. If that's "
                "your intention, pass `allow_module_level=True`. If you want to skip a specific "
                "test or an entire class, use the @pytest.mark.skip or @pytest.mark.skipif "
                "decorators."
            ) from exc
        except unittest.SkipTest as exc:
            raise _SkipCollect(str(exc)) from exc

    def _collect_module(self, module: Module) -> list[Function]:
        """Import a test module and collect its items; a failure is the
        module's collection error, a module-level skip skips it."""
        try:
            module.obj = self._import_test_module(module)
            self.consider_module(module.obj, module.aye_id)
            module.own_markers = get_unpacked_marks(module.obj)
            module.keywords.update((mark.name, mark) for mark in module.own_markers)
            self._register_xunit_module(module)
            self.parse_fixtures(module.obj, module)
            return self._collect_members(module)
        except _SkipCollect as skip:
            self.skipped_paths.append((module.aye_id, skip.reason))
            self.diagnostic("info", "skipped", module.aye_id, skip.reason or "skipped by pytest")
        except _ConftestFailed:
            self.failed_paths.append(module.aye_id)
        except COLLECT_ERROR as exc:
            self.failed_paths.append(module.aye_id)
            self.diagnostic("error", "collection-error", module.aye_id, str(exc))
        except (*OUTCOMES,) as exc:
            self.failed_paths.append(module.aye_id)
            message = (
                str(getattr(exc, "msg", None) or exc)
                if isinstance(exc, FAILED) and not getattr(exc, "pytrace", True)
                else _short_traceback(exc)
            )
            self.diagnostic("error", "collection-error", module.aye_id, message)
        return []

    def _collect_members(self, collector: Module | Class) -> list[Function]:
        """A module's or class's items, as pytest's ``PyCollector.collect``:
        each ``__dict__`` in definition order, a name once, inherited
        members first."""
        obj = collector.obj
        if not safe_getattr(obj, "__test__", True):
            return []
        dicts = [safe_getattr(obj, "__dict__", {})]
        if isinstance(obj, type):
            dicts += [base.__dict__ for base in obj.__mro__]
        seen: set[str] = set()
        groups: list[list[Function]] = []
        imported = self.config.getini("collect_imported_tests")
        for dic in dicts:
            values: list[Function] = []
            for name, member in list(dic.items()):
                if name in IGNORED_ATTRIBUTES or name in seen:
                    continue
                seen.add(name)
                if (
                    not imported
                    and isinstance(collector, Module)
                    and (inspect.isfunction(member) or inspect.isclass(member))
                    and member.__module__ != obj.__name__
                ):
                    continue
                values.extend(self._make_items(collector, name, member))
            groups.append(values)
        result: list[Function] = []
        for values in reversed(groups):
            result.extend(values)
        return result

    def _matches(self, option: str, name: str) -> bool:
        for pattern in self.config.getini(option):
            if name.startswith(pattern):
                return True
            if ("*" in pattern or "?" in pattern or "[" in pattern) and fnmatch.fnmatch(
                name, pattern
            ):
                return True
        return False

    def _make_items(  # noqa: PLR0911 - one return per kind of member
        self, collector: Module | Class, name: str, obj: Any
    ) -> list[Function]:
        if _is_testcase(obj):
            if inspect.isabstract(obj):
                return []
            return self._collect_class(UnitTestCase(self, collector, name, obj))
        if isinstance(obj, type):
            if (
                self._matches("python_classes", name)
                or safe_getattr(obj, "__test__", False) is True
            ) and not inspect.isabstract(obj):
                return self._collect_class(Class(self, collector, name, obj))
            return []
        if not self._istestfunction(obj, name):
            return []
        function = getattr(obj, "__func__", obj)
        if not (inspect.isfunction(function) or inspect.isfunction(get_real_func(function))):
            filename, line = getfslineno(function)
            warnings.warn_explicit(
                pytest.PytestCollectionWarning(
                    f"cannot collect {name!r} because it is not a function."
                ),
                category=None,
                filename=str(filename),
                lineno=line or 1,
            )
            return []
        if not safe_getattr(function, "__test__", True):
            return []
        if inspect.isgeneratorfunction(function):
            pytest.fail(
                f"'yield' keyword is allowed in fixtures, but not in tests ({name})", pytrace=False
            )
        return list(self._genfunctions(collector, name, function, obj))

    def _istestfunction(self, obj: Any, name: str) -> bool:
        if self._matches("python_functions", name) or safe_getattr(obj, "__test__", False) is True:
            if isinstance(obj, staticmethod | classmethod):
                obj = safe_getattr(obj, "__func__", False)
            return callable(obj) and _fixture_spec(obj) is None
        return False

    def _collect_class(self, node: Class) -> list[Function]:
        """A class's items. pytest collects each class on its own: an error
        fails the class (a diagnostic for its file), not its module."""
        try:
            return self._collect_class_items(node)
        except (_SkipCollect, _ConftestFailed):
            raise
        except COLLECT_ERROR as exc:
            self._class_failed(node, str(exc))
        except (*OUTCOMES,) as exc:
            message = (
                str(getattr(exc, "msg", None) or exc)
                if isinstance(exc, FAILED) and not getattr(exc, "pytrace", True)
                else _short_traceback(exc)
            )
            self._class_failed(node, message)
        return []

    def _class_failed(self, node: Class, message: str) -> None:
        file = node.aye_id.split("::", 1)[0]
        self.failed_paths.append(file)
        self.diagnostic("error", "collection-error", file, f"{node.aye_id}: {message}")

    def _collect_class_items(self, node: Class) -> list[Function]:
        cls = node.obj
        node.own_markers = get_unpacked_marks(cls)
        node.keywords.update((mark.name, mark) for mark in node.own_markers)
        if isinstance(node, UnitTestCase):
            return self._collect_testcase(node)
        if not safe_getattr(cls, "__test__", True):
            return []
        for attribute, what in (("__init__", _hasinit), ("__new__", _hasnew)):
            if what(cls):
                node.warn(
                    pytest.PytestCollectionWarning(
                        f"cannot collect test class {cls.__name__!r} because it has a "
                        f"{attribute} constructor (from: {node.parent.nodeid})"  # type: ignore[union-attr]
                    )
                )
                return []
        self._register_xunit_class(node)
        self.parse_fixtures(node.newinstance(), node, holder_class=cls)
        return self._collect_members(node)

    def _collect_testcase(self, node: UnitTestCase) -> list[Function]:
        """A unittest ``TestCase``: its methods by name
        (``getTestCaseNames``), ``runTest`` when it has none."""
        cls = node.obj
        if not getattr(cls, "__test__", True):
            return []
        if getattr(cls, "__unittest_skip__", False):
            self._register_unittest_skip(node)
        else:
            self._register_unittest_setup_method(node)
            self._register_unittest_setup_class(node)
            self._register_xunit_class(node, methods=False)
        self.parse_fixtures(node.newinstance(), node, holder_class=cls)
        items: list[Function] = []
        for name in unittest.TestLoader().getTestCaseNames(cls):
            if not getattr(getattr(cls, name), "__test__", True):
                continue
            items.append(TestCaseFunction(self, name, node, raw=inspect.getattr_static(cls, name)))
        if not items:
            run_test = getattr(cls, "runTest", None)
            if run_test is not None:
                items.append(
                    TestCaseFunction(
                        self, "runTest", node, raw=inspect.getattr_static(cls, "runTest")
                    )
                )
        return items

    def _genfunctions(
        self, collector: Module | Class, name: str, function: Any, raw: Any
    ) -> Iterator[Function]:
        module_node = collector.getparent(Module)
        module = module_node.obj
        cls = collector.obj if isinstance(collector, Class) else None
        definition = FunctionDefinition(self, name, collector, raw=raw)
        definition.obj = function
        definition._instance = None
        fixtureinfo = definition._fixtureinfo
        metafunc = Metafunc(self, definition, fixtureinfo, cls, module)
        methods = []
        if hasattr(module, "pytest_generate_tests"):
            methods.append(module.pytest_generate_tests)
        if cls is not None and hasattr(cls, "pytest_generate_tests"):
            methods.append(cls().pytest_generate_tests)
        self.pm.call(
            "pytest_generate_tests",
            {"metafunc": metafunc},
            directory=os.path.dirname(os.fspath(module_node.path)),
            extra=methods,
        )
        if not metafunc._calls:
            yield Function(self, name, collector, raw=raw, fixtureinfo=fixtureinfo)
            return
        metafunc._recompute_direct_params_indices()
        fixtureinfo.prune_dependency_tree()
        with no_automatic_gc():  # the items, by aye's code alone
            marks = get_unpacked_marks(_function_of(raw))
            items = [
                Function(
                    self,
                    f"{name}[{callspec.id}]" if callspec._idlist else name,
                    collector,
                    raw=raw,
                    callspec=callspec,
                    fixtureinfo=fixtureinfo,
                    keywords={callspec.id: True},
                    originalname=name,
                    directness=metafunc._params_directness,
                    marks=marks,
                )
                for callspec in metafunc._calls
            ]
        yield from items

    def parametersets(
        self, argnames: Any, argvalues: Any, function: Any, nodeid: str
    ) -> tuple[list[str], list[ParamSet]]:
        """pytest's ``ParameterSet._for_parametrize``."""
        if not isinstance(argvalues, Collection):
            warnings.warn(
                pytest.PytestRemovedIn10Warning(
                    "Passing a non-Collection iterable to parametrize is deprecated.\n"
                    f"Test: {nodeid}, argvalues type: {type(argvalues).__name__}\n"
                    "Please convert to a list or tuple.\n"
                    "See https://docs.pytest.org/en/stable/deprecations.html#parametrize-iterators"
                ),
                stacklevel=3,
            )
        names, force_tuple = _parse_parametrize_args(argnames)
        parameters = [_extract(value, force_tuple) for value in argvalues]
        if parameters:
            for param in parameters:
                if len(param.values) != len(names):
                    pytest.fail(
                        f'{nodeid}: in "parametrize" the number of names ({len(names)}):\n'
                        f"  {names}\nmust be equal to the number of values "
                        f"({len(param.values)}):\n  {param.values}",
                        pytrace=False,
                    )
        else:
            parameters.append(
                ParamSet((NOTSET,) * len(names), [self._empty_mark(names, function)], "NOTSET")
            )
        return names, parameters

    def _empty_mark(self, names: Sequence[str], function: Any) -> Any:
        reason = f"got empty parameter set for ({', '.join(names)})"
        requested = self.config.getini("empty_parameter_set_mark")
        if requested in ("", None, "skip"):
            return pytest.mark.skip(reason=reason).mark
        if requested == "xfail":
            return pytest.mark.xfail(reason=reason, run=False).mark
        if requested == "fail_at_collect":
            _, line = getfslineno(function)
            raise COLLECT_ERROR(f"Empty parameter set in '{function.__name__}' at line {line}")
        raise LookupError(requested)

    # ------------------------------------------------------------ fixtures

    def getfixtureinfo(self, node: Function, func: Any, cls: type | None) -> FuncFixtureInfo:
        """A test's fixtures: autouse ones (by node, outermost first), then
        ``usefixtures``, then its arguments; the closure sorted by scope."""
        if func is not None and not node.nofuncargs:
            argnames = getfuncargnames(func, name=node.name, cls=cls)
        else:
            argnames = ()
        usefixtures: list[str] = []
        for marker_node, mark in node.iter_markers_with_node(name="usefixtures"):
            if not mark.args:
                marker_node.warn(
                    pytest.PytestWarning(
                        f"usefixtures() in {node.nodeid} without arguments has no effect"
                    )
                )
            usefixtures.extend(mark.args)
        autouse = [name for parent in node.listchain() for name in self._autouse_names(parent)]
        initialnames = tuple(dict.fromkeys([*autouse, *usefixtures, *argnames]))
        direct = self._direct_parametrize_args(node)
        closure, arg2defs = self.getfixtureclosure(node.parent, initialnames, direct)
        return FuncFixtureInfo(argnames, initialnames, closure, arg2defs)

    def _autouse_names(self, node: NodeBase) -> list[str]:
        names = list(self.registry.autouse.get(id(node), ()))
        if node is self.session:
            names = [*self.config.getini("usefixtures"), *names]
        return names

    def _direct_parametrize_args(self, node: NodeBase) -> set[str]:
        names: set[str] = set()
        for marker in node.iter_markers(name="parametrize"):
            indirect = marker.kwargs.get("indirect", False)
            argnames = _parametrize_argnames(marker)
            directness = _resolve_args_directness(argnames, indirect, node.nodeid)  # type: ignore[attr-defined]
            names.update(name for name, kind in directness.items() if kind == "direct")
        return names

    def getfixtureclosure(
        self, parent: NodeBase | None, initialnames: tuple[str, ...], ignore: set[str]
    ) -> tuple[list[str], dict[str, Sequence[FixtureDef]]]:
        arg2defs: dict[str, Sequence[FixtureDef]] = {}

        def getdefs(name: str) -> Sequence[FixtureDef] | None:
            if name in ignore:
                return None
            defs = arg2defs.get(name)
            if not defs:
                defs = self.registry.visible(name, parent) if parent is not None else ()
                if not defs:
                    return None
                arg2defs[name] = defs
            return defs

        names = list(traverse_fixture_closure(initialnames, getdefs))
        names.sort(key=lambda n: arg2defs[n][-1].scope if n in arg2defs else Scope.FUNCTION)
        return names, arg2defs

    def parse_fixtures(
        self, holder: Any, node: NodeBase, *, holder_class: type | None = None
    ) -> None:
        """Register the fixtures ``holder`` defines (a module, a class
        instance, a plugin), in ``dir()`` order, visible from ``node``."""
        if inspect.isclass(holder) or isinstance(holder, types.ModuleType):
            holder_type: Any = holder
        else:
            holder_type = type(holder)
        for name in dir(holder):
            unbound = safe_getattr(holder_type, name, None)
            spec = _fixture_spec(unbound)
            wrapped = False
            if spec is None:
                # ``@pytest.fixture`` over ``@staticmethod`` (or
                # ``classmethod``): the spec is on the descriptor.
                try:
                    raw = inspect.getattr_static(holder_type, name, None)
                except Exception:  # noqa: BLE001 - an attribute that can't be read is no fixture
                    raw = None
                spec = _fixture_spec(raw) if isinstance(raw, staticmethod | classmethod) else None
                if spec is None:
                    continue
                wrapped = True
            fixture_name = spec.name or name
            try:
                bound = getattr(holder, name)
            except AttributeError:
                bound = unbound
            is_method = holder_class is not None and not wrapped
            self._register_fixture(
                fixture_name,
                unbound if is_method else bound,
                spec,
                node,
                signature=bound,
                is_method=is_method,
            )

    def _register_fixture(
        self,
        name: str,
        func: Any,
        spec: Any,
        node: NodeBase,
        *,
        signature: Any,
        is_method: bool,
    ) -> None:
        if name in self._blocked_fixtures and node is self.session:
            return
        scope = self._fixture_scope(name, spec.scope, func, node)
        argnames = getfuncargnames(signature, name=name)
        params = list(spec.params) if spec.params is not None else None
        filename, line = getfslineno(func)
        resource = ResourceDef(
            name=name,
            func=func,
            scope=scope,
            argnames=argnames,
            params=params,
            param_ids=self._param_ids(name, params, spec.ids),
            autouse=bool(spec.autouse),
            baseid=_baseid(node),
            is_method=is_method,
            location=f"{self.relative(filename)}:{line}" if filename else "",
        )
        self.registry.add(FixtureDef(node, resource, spec.ids), node)

    def _fixture_scope(self, name: str, scope: Any, func: Any, node: NodeBase) -> Scope:
        if callable(scope):
            try:
                scope = scope(fixture_name=name, config=self.config)
            except Exception as exc:
                raise TypeError(
                    f"Error evaluating {scope} while defining fixture '{name}'.\n"
                    "Expected a function with the signature (*, fixture_name, config)"
                ) from exc
            if not isinstance(scope, str):
                pytest.fail(
                    f"Expected {scope} to return a 'str' while defining fixture '{name}', but "
                    f"it returned:\n{scope!r}",
                    pytrace=False,
                )
        try:
            return Scope.named(scope)
        except (KeyError, AttributeError):
            pytest.fail(
                f"Fixture '{getattr(func, '__name__', name)}' from {_baseid(node) or '<root>'} got "
                f"an unexpected scope value '{scope}'",
                pytrace=False,
            )
            raise AssertionError from None  # unreachable: fail raises

    def _param_ids(self, name: str, params: list[Any] | None, ids: Any) -> list[str] | None:
        if params is None or callable(ids):
            return None
        try:
            sets = [ParamSet((value,), [], None) for value in params]
            made = IdMaker([name], sets, None, list(ids) if ids else None, None, None)
            return [str(i) for i in made.make_unique_parameterset_ids()]
        except Exception:  # noqa: BLE001 - informational: the tests' ids are made later
            return None

    def _add_internal(
        self,
        node: NodeBase,
        name: str,
        func: Any,
        scope: Scope,
        argnames: tuple[str, ...] = (),
    ) -> None:
        resource = ResourceDef(
            name=name,
            func=func,
            scope=scope,
            argnames=argnames,
            autouse=True,
            baseid=_baseid(node),
            location=f"{getattr(node, 'aye_id', '')}",
        )
        self.registry.add(FixtureDef(node, resource), node)

    def _register_xunit_module(self, node: Module) -> None:
        module = node.obj
        setup = _first_non_fixture(module, ("setUpModule", "setup_module"))
        teardown = _first_non_fixture(module, ("tearDownModule", "teardown_module"))
        if setup is not None or teardown is not None:

            def xunit_setup_module_fixture() -> Iterator[None]:
                if setup is not None:
                    _call_with_optional_argument(setup, module)
                yield
                if teardown is not None:
                    _call_with_optional_argument(teardown, module)

            self._add_internal(
                node,
                f"_xunit_setup_module_fixture_{module.__name__}",
                xunit_setup_module_fixture,
                Scope.MODULE,
            )
        setup_function = _first_non_fixture(module, ("setup_function",))
        teardown_function = _first_non_fixture(module, ("teardown_function",))
        if setup_function is None and teardown_function is None:
            return

        def xunit_setup_function_fixture(request: Any) -> Iterator[None]:
            if request.instance is not None:
                yield
                return
            function = request.function
            if setup_function is not None:
                _call_with_optional_argument(setup_function, function)
            yield
            if teardown_function is not None:
                _call_with_optional_argument(teardown_function, function)

        self._add_internal(
            node,
            f"_xunit_setup_function_fixture_{module.__name__}",
            xunit_setup_function_fixture,
            Scope.FUNCTION,
            ("request",),
        )

    def _register_xunit_class(self, node: Class, *, methods: bool = True) -> None:
        cls = node.obj
        setup = _first_non_fixture(cls, ("setup_class",))
        teardown = _first_non_fixture(cls, ("teardown_class",))
        if setup is not None or teardown is not None:

            def xunit_setup_class_fixture() -> Iterator[None]:
                if setup is not None:
                    _call_with_optional_argument(getattr(setup, "__func__", setup), cls)
                yield
                if teardown is not None:
                    _call_with_optional_argument(getattr(teardown, "__func__", teardown), cls)

            self._add_internal(
                node,
                f"_xunit_setup_class_fixture_{cls.__qualname__}",
                xunit_setup_class_fixture,
                Scope.CLASS,
            )
        if not methods:
            return
        setup_method = _first_non_fixture(cls, ("setup_method",))
        teardown_method = _first_non_fixture(cls, ("teardown_method",))
        if setup_method is None and teardown_method is None:
            return

        def xunit_setup_method_fixture(request: Any) -> Iterator[None]:
            instance = request.instance
            method = request.function
            if setup_method is not None:
                _call_with_optional_argument(instance.setup_method, method)
            yield
            if teardown_method is not None:
                _call_with_optional_argument(instance.teardown_method, method)

        self._add_internal(
            node,
            f"_xunit_setup_method_fixture_{cls.__qualname__}",
            xunit_setup_method_fixture,
            Scope.FUNCTION,
            ("request",),
        )

    def _register_unittest_setup_class(self, node: UnitTestCase) -> None:
        cls = node.obj
        setup = getattr(cls, "setUpClass", None)
        teardown = getattr(cls, "tearDownClass", None)
        if setup is None and teardown is None:
            return
        cleanup = getattr(cls, "doClassCleanups", lambda: None)

        def teardown_exceptions() -> None:
            infos = getattr(cls, "tearDown_exceptions", None)
            if not infos:
                return
            errors = [exc for (_, exc, _) in infos]
            if len(errors) == 1:
                raise errors[0]
            group = getattr(builtins, "ExceptionGroup", None)
            if group is None:
                raise errors[0]
            raise group("Unittest class cleanup errors", errors)

        def unittest_setup_class_fixture() -> Iterator[None]:
            if getattr(cls, "__unittest_skip__", False):
                raise SKIPPED(getattr(cls, "__unittest_skip_why__", ""))
            if setup is not None:
                try:
                    setup()
                except Exception:
                    cleanup()
                    teardown_exceptions()
                    raise
            yield
            try:
                if teardown is not None:
                    teardown()
            finally:
                cleanup()
                teardown_exceptions()

        self._add_internal(
            node,
            f"_unittest_setUpClass_fixture_{cls.__qualname__}",
            unittest_setup_class_fixture,
            Scope.CLASS,
        )

    def _register_unittest_skip(self, node: UnitTestCase) -> None:
        cls = node.obj

        def unittest_skip_fixture() -> None:
            raise SKIPPED(getattr(cls, "__unittest_skip_why__", ""))

        self._add_internal(
            node, f"_unittest_skip_fixture_{cls.__qualname__}", unittest_skip_fixture, Scope.CLASS
        )

    def _register_unittest_setup_method(self, node: UnitTestCase) -> None:
        cls = node.obj
        setup = getattr(cls, "setup_method", None)
        teardown = getattr(cls, "teardown_method", None)
        if setup is None and teardown is None:
            return

        def unittest_setup_method_fixture(request: Any) -> Iterator[None]:
            instance = request.instance
            if getattr(instance, "__unittest_skip__", False):
                raise SKIPPED(getattr(instance, "__unittest_skip_why__", ""))
            if setup is not None:
                setup(instance, request.function)
            yield
            if teardown is not None:
                teardown(instance, request.function)

        self._add_internal(
            node,
            f"_unittest_setup_method_fixture_{cls.__qualname__}",
            unittest_setup_method_fixture,
            Scope.FUNCTION,
            ("request",),
        )

    def _register_package_setup(self, node: Package) -> None:
        """A package's ``setup_module``/``teardown_module`` (in its
        ``__init__.py``), which pytest calls when its first test starts:
        an auto resource of the package's scope."""
        init = os.path.join(os.fspath(node.path), "__init__.py")
        try:
            module = self.import_path(init)
        except (*OUTCOMES,):
            return
        node.obj = module
        setup = _first_non_fixture(module, ("setUpModule", "setup_module"))
        teardown = _first_non_fixture(module, ("tearDownModule", "teardown_module"))
        if setup is None and teardown is None:
            return

        def package_setup() -> Iterator[None]:
            if setup is not None:
                _call_with_optional_argument(setup, module)
            yield
            if teardown is not None:
                _call_with_optional_argument(teardown, module)

        self._add_internal(
            node, f"_aye_package_setup_{module.__name__}", package_setup, Scope.PACKAGE
        )

    # ------------------------------------------------------------ the model

    def _model(self, node: NodeBase | None) -> aye_model.Node | None:
        """The model node of a package, module or class (directories
        without ``__init__.py`` and the session have none)."""
        while node is not None and node.model_kind is None:
            node = node.parent
        if node is None:
            return None
        model = self._models.get(id(node))
        if model is None:
            model = aye_model.Node(
                id=node.aye_id,  # type: ignore[attr-defined]
                kind=node.model_kind,
                obj=getattr(node, "obj", None),
                parent=self._model(node.parent),
                tags=[_tag(mark) for mark in node.own_markers],
            )
            self._models[id(node)] = model
            self._owners[id(model)] = node
        return model

    def _fill_visible(self) -> None:
        """Every model node's ``visible``: each resource name visible from
        it, closest definition first."""
        names = list(self.registry.defs)
        for model in self._models.values():
            node = self._owners[id(model)]
            chain = {id(n) for n in node.iter_parents()}
            visible: dict[str, list[ResourceDef]] = {}
            for name in names:
                defs = [fd.resource for fd in self.registry.defs[name] if id(fd.node) in chain]
                if defs:
                    visible[name] = defs[::-1]
            model.visible = visible

    def _cases(self, items: Iterable[Function]) -> list[Case]:
        """Each item's case (``_case``), in order: what items share (their
        parent's marks, their function's direct parameters, their kind) read
        once (nothing changes the items meanwhile)."""
        shared: dict[Any, Any] = {}
        return [self._case(item, shared) for item in items]

    def _case(self, item: Function, shared: dict[Any, Any] | None = None) -> Case:
        """The model's case for ``item``. ``shared`` keeps what the items
        before it share with it (``_cases``)."""
        if shared is None:
            shared = {}
        callspec: CallSpec2 | None = getattr(item, "callspec", None)
        call_params: dict[str, Any] = {}
        resource_params: dict[str, tuple[int, Any, str]] = {}
        param_scopes: dict[str, Scope] = {}
        if callspec is not None:
            directness = item._directness
            found = shared.get(("direct", id(directness)))
            if found is None or found[0] is not directness:
                direct = frozenset(name for name, kind in directness.items() if kind == "direct")
                found = shared["direct", id(directness)] = (directness, direct)
            direct = found[1]
            arg2scope, indices, ids = callspec._arg2scope, callspec.indices, callspec._ids
            for name, value in callspec.params.items():
                param_scopes[name] = arg2scope[name]
                if name in direct:
                    call_params[name] = value
                else:
                    resource_params[name] = (indices[name], value, ids.get(name, ""))
        parent = item.parent
        found = shared.get(("tags", id(parent)))
        if found is None or found[0] is not parent:
            above = [self._tag(mark) for mark in parent.iter_markers()] if parent else []
            found = shared["tags", id(parent)] = (parent, above)
        # pytest's ``iter_markers``: the item's own marks, then its parents'.
        tags = [self._tag(mark) for mark in item.own_markers]
        tags += found[1]
        kind = shared.get(("kind", type(item)))
        if kind is None:
            kind = "unittest" if isinstance(item, TestCaseFunction) else "function"
            shared["kind", type(item)] = kind
        fixturenames = item.fixturenames
        key = ("closure", id(fixturenames), *call_params)
        found = shared.get(key)
        if found is None or found[0] is not fixturenames:
            closure = tuple(name for name in fixturenames if name not in call_params)
            found = shared[key] = (fixturenames, closure)
        case = Case(
            id=item.aye_id,
            node=self._model(parent),  # type: ignore[arg-type]
            func=self._case_func(item),
            cls=item.cls,
            closure=found[1],
            argnames=tuple(item._fixtureinfo.argnames),
            call_params=call_params,
            resource_params=resource_params,
            param_scopes=param_scopes,
            tags=tags,
            # The names ``-k`` matches are the refinement's (``case_dict``):
            # the engine reads the item's own.
            location=self._location(item),
            kind=kind,
            extra={"item": item, "config": self.config, "session": self.session},
        )
        item._case = case
        return case

    def _location(self, item: Function) -> tuple[str, int]:
        """Where the item's function is defined (relative path, 1-based
        line), once per function."""
        holder = item._marks_holder()
        found = self._locations.get(id(holder))
        if found is None or found[0] is not holder:
            filename, line = getfslineno(holder)
            where = (self.relative(filename) if filename else item.aye_id.split("::")[0], line)
            found = self._locations[id(holder)] = (holder, where)
        return found[1]

    def _tag(self, mark: Any) -> Tag:
        """A mark as the model's tag, once per mark."""
        found = self._mark_tags.get(id(mark))
        if found is None or found[0] is not mark:
            found = self._mark_tags[id(mark)] = (mark, _tag(mark))
        return found[1]

    def _case_func(self, item: Function) -> Any:
        if item._obj is not NOTSET and item.cls is None:
            return item._obj
        if item.cls is not None:
            return item._raw
        return getattr(item.parent.obj, item.originalname)  # type: ignore[union-attr]

    def _hooks(self) -> Hooks:
        """The conftests' ``pytest_runtest_setup``, ``_call`` and
        ``_teardown`` (path-sensitive: a conftest's only for the tests below
        it), and, at teardown, what ``item.addfinalizer`` registered."""
        hooks = Hooks()
        if self.pm.impls("pytest_runtest_setup"):
            hooks.setup.append(self._phase_hook("pytest_runtest_setup"))
        if self.pm.impls("pytest_runtest_call"):
            hooks.call.append(self._phase_hook("pytest_runtest_call"))
        self._teardown_impls = bool(self.pm.impls("pytest_runtest_teardown"))
        hooks.teardown.append(self._teardown_hook)
        return hooks

    def _phase_hook(self, name: str) -> Callable[[Case], Any]:
        def call(case: Case) -> None:
            item = case.extra["item"]
            kwargs = {"item": item, "nextitem": None}
            self.pm.call(name, kwargs, directory=_directory_of(item))

        call.__name__ = name
        return call

    def _teardown_hook(self, case: Case) -> None:
        """``pytest_runtest_teardown``, then the item's finalizers, last in,
        first out: every one runs, the first error is raised."""
        item = case.extra.get("item")
        errors: list[BaseException] = []
        if item is not None and self._teardown_impls:
            try:
                self.pm.call(
                    "pytest_runtest_teardown",
                    {"item": item, "nextitem": None},
                    directory=_directory_of(item),
                )
            except Exception as exc:  # noqa: BLE001 - the finalizers still run
                errors.append(exc)
        finalizers = getattr(item, "_finalizers", None)
        while finalizers:
            try:
                finalizers.pop()()
            except Exception as exc:  # noqa: BLE001 - every finalizer runs; the first is raised
                errors.append(exc)
        if errors:
            raise errors[0]

    # ------------------------------------------------------------ the end

    def finish(self, exitstatus: int) -> list[dict[str, Any]]:
        """``pytest_sessionfinish``, ``pytest_unconfigure``, the config's
        cleanups and the session's finalizers. Failures are diagnostics."""
        if not self._started:
            return []
        found: list[dict[str, Any]] = []
        self.session.exitstatus = exitstatus
        for name, kwargs in (
            ("pytest_sessionfinish", {"session": self.session, "exitstatus": exitstatus}),
            ("pytest_unconfigure", {"config": self.config}),
        ):
            logs = self.logging_plugin() if name == "pytest_sessionfinish" else nullcontext()
            try:
                with logs:
                    self.pm.call(name, kwargs)
            except (*OUTCOMES,) as exc:
                found.append(
                    {
                        "severity": "error",
                        "code": "internal-error",
                        "path": ".",
                        "message": f"{name} raised:\n{_short_traceback(exc)}",
                    }
                )
        cleanups = [*self.config._cleanups, *self._node_finalizers()]
        while cleanups:
            try:
                cleanups.pop()()
            except (*OUTCOMES,) as exc:
                found.append(
                    {
                        "severity": "error",
                        "code": "internal-error",
                        "path": ".",
                        "message": f"a cleanup raised:\n{_short_traceback(exc)}",
                    }
                )
        mark = pytest.mark
        if getattr(mark, "_config", None) is self.config:
            mark._config = None
        if getattr(mark, "_markers_of", None) is self.config:
            mark._markers_of = None
        return found

    def _node_finalizers(self) -> list[Callable[[], Any]]:
        nodes: list[NodeBase] = [self.session, *self._dir_nodes.values()]
        nodes += list(self._owners.values())
        found = []
        for node in nodes:
            if node._finalizers:
                found += node._finalizers
                node._finalizers = []
        return found

    # ------------------------------------------------------------ refine output

    def case_dicts(self, cases: Iterable[Case]) -> list[dict[str, Any]]:
        """Each case's ``case_dict``, in order: what the names ``-k`` matches
        owe to a test's class or module and to its function is read once
        for the tests that share them (nothing changes the nodes
        meanwhile)."""
        shared: dict[tuple[int, int], Any] = {}
        return [self.case_dict(case, shared) for case in cases]

    def case_dict(
        self, case: Case, shared: dict[tuple[int, int], Any] | None = None
    ) -> dict[str, Any]:
        """A case as a refinement reports it (the wire format of the pytest
        adapter's refined cases, with this front-end's ``affinity``). Its
        texts are the names and ids collection made, which the worker's
        encoder carries as they are. ``shared`` holds what ``keywords``
        read for the cases before it (``case_dicts``)."""
        data: dict[str, Any] = {"id": case.id, "adapter": "pytest"}
        path, line = case.location
        span: dict[str, Any] = {"path": path, "start": 0, "end": 0}
        if line:
            span["line"] = line
        data["span"] = span
        tags = [tag.name for tag in case.tags]
        if tags:
            data["tags"] = tags if len(tags) == 1 else list(dict.fromkeys(tags))
        item = case.extra.get("item")
        names = case.keywords or (keywords(item, shared) if isinstance(item, Function) else [])
        if names:
            data["keywords"] = names
        data["affinity"] = self.affinity(case)
        timeout = _timeout(case)
        if timeout is not None:
            data["timeout"] = timeout
        data["certainty"] = "refined"
        return data

    def affinity(self, case: Case) -> list[str]:
        """The lifetimes ``case`` must share with others, outermost first,
        which the runner keeps in one batch: none, unless it shares a costly
        one. ``module:<path>`` when its module has setup that runs user code
        for it (a module- or package-scoped resource in its closure, which
        ``setup_module`` and package setups are), then ``class:<id>`` for its
        class when that has some (a class-scoped resource, ``setup_class``,
        a ``TestCase``'s own ``setUpClass``, ``tearDownClass`` or class
        cleanups). A file whose module name another test directory holds
        too (the context's ``colliding_modules``: pytest's import file
        mismatch, or a helper Python imports once) keeps its ``import:NAME``
        key first. Session-scoped resources share nothing: every worker has
        its own session, as under pytest-xdist."""
        file = case.id.partition("::")[0]
        keys = []
        grouped = self._grouping_key(file)
        if grouped is not None:
            keys.append(grouped)
        item = case.extra.get("item")
        if not isinstance(item, Function):
            return keys
        module_wide, class_wide = self._costly_scopes(item, case)
        if module_wide or class_wide:
            keys.append(f"module:{file}")
        if class_wide:
            keys += [f"class:{node.aye_id}" for node in item.listchain() if isinstance(node, Class)]
        return keys

    def _costly_scopes(self, item: Function, case: Case) -> tuple[bool, bool]:
        """Whether a resource ``case`` uses lives as long as its module (or
        package), and as long as its class (when it has one)."""
        module_wide = class_wide = False
        defs = item._fixtureinfo.name2fixturedefs
        for name in case.closure:
            chain = defs.get(name)
            if not chain:
                continue
            used = len(chain) - 1
            while True:
                definition = chain[used]
                scope = definition.scope
                if used == len(chain) - 1 and name in case.param_scopes:
                    scope = case.param_scopes[name]
                if scope in (Scope.MODULE, Scope.PACKAGE):
                    module_wide = True
                elif (
                    scope is Scope.CLASS
                    and case.cls is not None
                    and self._runs_class_code(definition, case.cls)
                ):
                    class_wide = True
                # A definition that requests its own name uses the one it
                # overrides.
                if used == 0 or name not in definition.argnames:
                    break
                used -= 1
        return module_wide, class_wide

    @staticmethod
    def _runs_class_code(definition: FixtureDef, cls: type) -> bool:
        """Whether a class-scoped resource runs the user's code: any but the
        one standing for a ``TestCase``'s ``setUpClass`` and
        ``tearDownClass`` when the class has neither of its own, nor class
        cleanups (and the one that skips a skipped ``TestCase``)."""
        name = definition.name
        if name.startswith("_unittest_skip_fixture_"):
            return False
        if not name.startswith("_unittest_setUpClass_fixture_"):
            return True
        return _own_class_code(cls)

    def _grouping_key(self, file: str) -> str | None:
        """``file``'s ``import:NAME`` key when the import-mismatch rule
        groups it (its module name, or a helper it imports, is one of the
        context's ``colliding_modules``), else ``None``; once per file."""
        found = self._grouping_keys.get(file, NOTSET)
        if found is NOTSET:
            colliding = {
                m for m in self.context.get("colliding_modules") or () if isinstance(m, str)
            }
            found = None
            if colliding:
                key = self.import_key(file)
                if key.partition(":")[2].split(".")[0] in colliding:
                    found = key
            self._grouping_keys[file] = found
        return found

    def import_key(self, file: str) -> str:
        path = self.absolute(file)
        key = import_key(
            path,
            self._import_mode,
            self.settings.rootdir,
            bool(self.config.getini("consider_namespace_packages")),
        )
        colliding = [m for m in self.context.get("colliding_modules") or () if isinstance(m, str)]
        helper = _colliding_import(path, colliding) if colliding else None
        return f"import:{helper}" if helper is not None else key


# ------------------------------------------------------------------ module helpers


def _baseid(node: NodeBase) -> str:
    return getattr(node, "baseid", "") or ""


def _directory_of(item: Function) -> str:
    """The directory of the item's module, for path-sensitive hook calls
    (once per module)."""
    module = item.parent
    while module is not None and not isinstance(module, Module):
        module = module.parent
    holder: Any = module if module is not None else item
    found = holder.__dict__.get("_aye_directory")
    if found is None:
        found = holder._aye_directory = os.path.dirname(os.fspath(item.module_path))
    return found


def _resolve_pkg_root_and_module_name(path: str, namespace: bool) -> tuple[str, str] | None:
    """pytest's ``resolve_pkg_root_and_module_name``: the directory above
    ``path``'s outermost package (with ``consider_namespace_packages``, the
    first directory upward from which its name imports it) and its dotted
    name; ``None`` outside packages."""
    package = resolve_package_path(path)
    root = os.path.dirname(package) if package is not None else None
    if namespace:
        candidate = root if root is not None else os.path.dirname(path)
        while True:
            name = compute_module_name(candidate, path)
            if name and _is_importable(name, path):
                root = candidate
                break
            parent = os.path.dirname(candidate)
            if parent == candidate:
                break
            candidate = parent
    if root is not None:
        name = compute_module_name(root, path)
        if name:
            return root, name
    return None


def _spec_matches(spec: Any, path: str) -> bool:
    if spec is None:
        return False
    if spec.origin:
        return os.path.normpath(spec.origin) == os.path.normpath(path)
    return any(
        os.path.normpath(location) == os.path.normpath(path)
        for location in spec.submodule_search_locations or ()
    )


def _import_module_using_spec(name: str, path: str, *, insert_modules: bool) -> Any:
    """pytest's ``_import_module_using_spec``: the parent package first
    (from its ``__init__.py``), then the module, through ``sys.meta_path``
    (aye's rewriting finder included) or by its file."""
    parent_name, _, child = name.rpartition(".")
    parent: Any = None
    if parent_name:
        parent = sys.modules.get(parent_name)
        if parent is None or not hasattr(parent, "__path__"):
            parent = _import_parent(parent_name, path, insert_modules=insert_modules)
    spec = _module_spec(name, child, path)
    if not _spec_matches(spec, path):
        return None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
    except BaseException:
        sys.modules.pop(name, None)
        raise
    if parent is not None:
        setattr(parent, child, module)
    if insert_modules:
        _insert_missing_modules(name)
    return module


def _import_parent(parent_name: str, path: str, *, insert_modules: bool) -> Any:
    """The package (or directory) above the module at ``path``."""
    if os.path.basename(path) == "__init__.py":
        parent_path = os.path.dirname(os.path.dirname(path))
    else:
        parent_path = os.path.dirname(path)
    if os.path.isfile(os.path.join(parent_path, "__init__.py")):
        parent_path = os.path.join(parent_path, "__init__.py")
    return _import_module_using_spec(parent_name, parent_path, insert_modules=insert_modules)


def _module_spec(name: str, child: str, path: str) -> Any:
    """The spec ``sys.meta_path`` finds for ``name`` at ``path``, else one
    made from the file (a namespace loader for a directory)."""
    search = [os.path.dirname(path)]
    for finder in sys.meta_path:
        find_spec = getattr(finder, "find_spec", None)
        spec = find_spec(name, search) if find_spec is not None else None
        if _spec_matches(spec, path):
            return spec
    loader = None
    if os.path.isdir(path):
        namespace_loader = getattr(importlib.machinery, "NamespaceLoader", None)
        if namespace_loader is not None:
            loader = namespace_loader(child, [path], importlib.machinery.PathFinder())
    return importlib.util.spec_from_file_location(name, path, loader=loader)


def _insert_missing_modules(name: str) -> None:
    """pytest's ``insert_missing_modules``: the parents of ``name``,
    imported or made empty, each holding its child."""
    parts = name.split(".")
    while name:
        parent_name, _, child = name.rpartition(".")
        if parent_name:
            parent = sys.modules.get(parent_name)
            if parent is None:
                try:
                    if not sys.meta_path:
                        raise ModuleNotFoundError
                    parent = importlib.import_module(parent_name)
                except ModuleNotFoundError:
                    parent = types.ModuleType(
                        name, doc="Empty module created by pytest's importmode=importlib."
                    )
                sys.modules[parent_name] = parent
            if not hasattr(parent, child):
                setattr(parent, child, sys.modules[name])
        parts.pop(-1)
        name = ".".join(parts)


def _is_importable(name: str, path: str) -> bool:
    try:
        spec = importlib.util.find_spec(name)
    except (ImportError, ValueError, ImportWarning):
        return False
    if spec is None or spec.origin is None:
        return False
    return _same_file(spec.origin, path)


def parse_warning_filter(arg: str, *, escape: bool) -> tuple[Any, str, type[Warning], str, int]:
    """A ``-W``/``filterwarnings`` line as ``warnings.filterwarnings``
    arguments (pytest's ``parse_warning_filter``)."""
    parts = arg.split(":")
    if len(parts) > 5:  # noqa: PLR2004 - action:message:category:module:line
        raise pytest.UsageError(
            f"while parsing the following warning configuration:\n\n  {arg}\n\n"
            f"Too many fields ({len(parts)}), expected at most 5 separated by colons"
        )
    while len(parts) < 5:  # noqa: PLR2004
        parts.append("")
    action_, message, category_, module, lineno_ = (s.strip() for s in parts)
    actions = {"default", "error", "ignore", "always", "all", "module", "once"}
    matches = [a for a in actions if a.startswith(action_)] if action_ else ["default"]
    if len(matches) != 1:
        raise pytest.UsageError(f"invalid action: {action_!r} (in {arg!r})")
    action = "always" if matches[0] == "all" else matches[0]
    category = _resolve_warning_category(category_)
    if message and escape:
        message = re.escape(message)
    if module and escape:
        module = re.escape(module) + r"\Z"
    lineno = 0
    if lineno_:
        try:
            lineno = int(lineno_)
            if lineno < 0:
                raise ValueError
        except ValueError:
            raise pytest.UsageError(f"invalid lineno {lineno_!r} (in {arg!r})") from None
    return action, message, category, module, lineno


def _resolve_warning_category(category: str) -> type[Warning]:
    if not category:
        return Warning
    if "." not in category:
        module: Any = builtins
        name = category
    else:
        modname, _, name = category.rpartition(".")
        module = __import__(modname, None, None, [name])
    found = getattr(module, name)
    if not (isinstance(found, type) and issubclass(found, Warning)):
        raise pytest.UsageError(f"{found} is not a Warning subclass")
    return found


def keywords(item: Function, shared: dict[tuple[int, int], Any] | None = None) -> list[str]:
    """The names pytest's ``-k`` matches: the chain's names (but the
    session's and the top directory's), extra keywords, the function's
    attributes and the marks, sorted. What the item's parents and its
    function give is read once per pair of them when ``shared`` keeps it
    (``Frontend.case_dicts``); the item's own names are added to it."""
    parent = item.parent
    holder = item._marks_holder()
    key = (id(parent), id(holder))
    found = shared.get(key) if shared is not None else None
    if found is None:
        names = set(_chain_names(parent)) if parent is not None else set()
        if parent is not None:
            for node in parent.iter_parents():
                names.update(node._aye_extra_keywords or ())
            names.update(mark.name for mark in parent.iter_markers())
        names.update(safe_getattr(holder, "__dict__", {}))
        # The parent and the function stay referenced: their ids stay theirs.
        found = (frozenset(names), sorted(names), parent, holder)
        if shared is not None:
            shared[key] = found
    common = found[0]
    names = list(found[1])
    own = [item.name]
    extra = item._aye_extra_keywords
    if extra:
        own.extend(extra)
    for mark in item.own_markers:
        own.append(mark.name)
    for name in own:
        if name not in common:
            at = bisect_left(names, name)
            if at == len(names) or names[at] != name:
                names.insert(at, name)
    return names


def _chain_names(node: NodeBase) -> frozenset[str]:
    """The names of ``node`` and its parents but the session and the top
    directory (``keywords``), once per node."""
    found = node.__dict__.get("_aye_names")
    if found is None:
        if isinstance(node, Session):
            found = frozenset()
        else:
            above = _chain_names(node.parent) if node.parent is not None else frozenset()
            top = isinstance(node, DirectoryBase) and isinstance(node.parent, Session)
            found = above if top else above | {node.name}
        node._aye_names = found  # type: ignore[attr-defined]
    return found


def _timeout(case: Case) -> float | None:
    tag = case.tag("timeout")
    if tag is None:
        return None
    value = tag.args[0] if tag.args else tag.kwargs.get("timeout")
    if isinstance(value, bool):
        return None
    try:
        timeout = float(value)
    except (TypeError, ValueError, OverflowError):
        return None
    return timeout if math.isfinite(timeout) and timeout > 0 else None


_CLASS_CODE = ("setUpClass", "tearDownClass", "doClassCleanups")
_CLASS_CLEANUPS = frozenset({"addClassCleanup", "enterClassContext"})


def _own_class_code(cls: type) -> bool:
    """Whether a ``TestCase`` class runs code of its own around its tests:
    a ``setUpClass``, ``tearDownClass`` or ``doClassCleanups`` other than
    unittest's, or a method that registers class cleanups."""
    for base in cls.__mro__:
        if base.__module__ == "builtins" or base.__module__.startswith("unittest."):
            continue
        namespace = vars(base)
        if any(name in namespace for name in _CLASS_CODE):
            return True
        for member in namespace.values():
            function = getattr(member, "__func__", member)
            code = getattr(function, "__code__", None)
            if code is not None and _CLASS_CLEANUPS.intersection(code.co_names):
                return True
    return False


def import_key(path: str, import_mode: str, rootdir: str, namespace_packages: bool) -> str:
    """``import:NAME``, NAME being the module name pytest imports the test
    file at ``path`` under (the pytest adapter's recognizer computes the
    same key)."""
    stem = os.path.splitext(os.path.basename(path))[0]
    if namespace_packages:
        return f"import:{stem}"
    package = resolve_package_path(path)
    name = compute_module_name(os.path.dirname(package), path) if package is not None else None
    if name is None and import_mode == "importlib":
        name = module_name_from_path(path, rootdir)
    return f"import:{stem if name is None else name}"


def _colliding_import(path: str, colliding: list[str]) -> str | None:
    """The smallest of ``colliding`` that the module at ``path`` imports
    from its own import base (see the pytest adapter's ``_colliding_import``)."""
    base = os.path.dirname(path)
    while os.path.isfile(os.path.join(base, "__init__.py")):
        base = os.path.dirname(base)
    try:
        with open(path, "rb") as source:
            tree = ast.parse(source.read())
    except (OSError, SyntaxError, ValueError):
        return None
    names: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            names.update(alias.name.split(".")[0] for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            names.add(node.module.split(".")[0])
    local = sorted(
        name
        for name in names.intersection(colliding)
        if os.path.isfile(os.path.join(base, f"{name}.py"))
        or os.path.isfile(os.path.join(base, name, "__init__.py"))
    )
    return local[0] if local else None
