"""The doctest adapter's executor: doctests run natively on aye's engine.

The worker installs this module as ``aye_engine_doctest``, after
``aye_engine``. doctest maps fully onto aye's engine: each ``DocTest`` (a
docstring with examples, or a text file) is one ``aye_engine.Test``, and
``EngineExecutor`` runs the loop. Ids and outcomes follow pytest's doctest
plugin, so plain pytest (``--doctest-modules``, ``--doctest-glob``) is the
referee.

- Ids: ``path::name``, where ``name`` is what ``DocTestFinder`` names a test
  (``pkg.mod``, ``pkg.mod.func``, ``pkg.mod.Class.method``) and a text
  file's name is its file name.
- Modules are imported as pytest's default import mode imports them: the
  package root (or the file's directory) first on ``sys.path``, then
  ``importlib.import_module`` and pytest's "import file mismatch" check.
  ``importlib`` mode goes through pytest's own ``import_path`` when pytest is
  installed.
- Every module and text file a batch names is loaded and searched before
  any test runs, as pytest collects before running: after the conftests
  pytest would have imported by then (when pytest is installed: its
  initial ones are the context's, or, when the command line selects by
  more than paths, those of the paths the binary's ``selected_paths``
  names), under pytest's warning filters, with ``os.path``'s own functions
  in place. A
  module that fails to import, or a docstring doctest can't parse, is a
  ``collection-error`` diagnostic and its tests don't run; a module skipped
  while importing finishes its tests as ``skipped``. A conftest that raises
  pytest's ``skip`` while loaded skips the files below it, as pytest skips
  its directory (a ``skipped`` info on the directory in refinement); one
  that fails is one diagnostic on its own path (a ``usage-error`` for one
  pytest loads at start-up, else a ``collection-error``), and the files
  below it don't run.
- Every example runs, even after a failure, and each one is a
  ``sub_outcome``. The test is ``skipped`` when every example has its own
  ``+SKIP`` (none runs), ``failed`` when any example failed, else
  ``passed``, as in pytest. pytest's ``skip``, ``xfail`` and ``fail`` raised
  in an example end the test as pytest ends it, unless an example failed
  before (without ``--doctest-continue-on-failure``, pytest stops there).
  Examples run under pytest's warning filters, with ``PYTEST_VERSION`` and
  ``PYTEST_CURRENT_TEST`` set when pytest is installed.
- A mismatch is a structured failure (``got == expected``, with a unified
  diff, blank output lines shown as ``<BLANKLINE>``); an unexpected
  exception carries its traceback, the example's frame mapped to the file
  and line it is written on. A failure with no line has the test and the
  example's position as its signature's place (``Failure.site``).
- pytest's checker options ``ALLOW_UNICODE``, ``ALLOW_BYTES`` and ``NUMBER``
  are registered and implemented here, without pytest.
- A test with a bad outcome carries its ``reproduce`` (``EngineExecutor``
  sets it): ``aye``, aye's own options that shaped the run (the context's
  ``rerun_args``), the id, then ``--`` and the batch's arguments after it,
  without ``--junitxml``, whose report a rerun would overwrite.
- Durations come from ``aye_engine.perf_counter``, capture from
  ``aye_engine.CaptureIO`` and paths from ``aye_engine.os_path``, all bound
  at import: an example that patches ``time``, ``io`` or ``os.path``
  changes nothing the executor reports.

It imports only the standard library, runs on Python 3.10 and later, with or
without pytest, and never writes to stdout, which belongs to the worker.
"""

from __future__ import annotations

import builtins
import difflib
import doctest
import functools
import importlib
import inspect
import math
import os
import re
import sys
import warnings
from collections import Counter
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from linecache import getline
from pathlib import Path
from traceback import extract_tb, format_exception_only, format_list
from typing import Any

import aye_engine
from aye_engine import Failure, Outcome, Phase, Result, Services, Sink, perf_counter

# ``os.path`` as it was when the engine was imported (``aye_engine.os_path``):
# an example may patch ``os.path`` and leave it patched.
_path = aye_engine.os_path
_listdir = os.listdir

# pytest's checker options, registered as pytest registers them.
ALLOW_UNICODE = doctest.register_optionflag("ALLOW_UNICODE")
ALLOW_BYTES = doctest.register_optionflag("ALLOW_BYTES")
NUMBER = doctest.register_optionflag("NUMBER")

# The flags under which an example's output may match what it expects
# without being equal to it, whatever it expects.
_LOOSE_MATCHING = doctest.NORMALIZE_WHITESPACE | ALLOW_UNICODE | ALLOW_BYTES | NUMBER

# ``--doctest-report``'s choices, as doctest flags.
REPORTS = {
    "udiff": doctest.REPORT_UDIFF,
    "cdiff": doctest.REPORT_CDIFF,
    "ndiff": doctest.REPORT_NDIFF,
    "only_first_failure": doctest.REPORT_ONLY_FIRST_FAILURE,
    "none": 0,
}

# A ``batch-stopped`` diagnostic names at most this many tests.
MAX_NAMED = 20

# What pytest says when every example is skipped.
ALL_SKIPPED = "all tests skipped by +SKIP option"

# pytest's options that write a JUnit report: not part of a rerun.
JUNITXML = ("--junitxml", "--junit-xml")

_DOCTEST_FRAME = re.compile(r"^<doctest (?P<name>.+)\[(?P<index>\d+)\]>$")


def _internal_fragments() -> tuple[str, ...]:
    """doctest's own file, plus this module and aye's engine when they were
    loaded from files (the worker installs them from memory, as ``<aye:…>``,
    which ``Paths`` already hides)."""
    fragments = [getattr(doctest, "__file__", None)]
    for code in (_internal_fragments.__code__, aye_engine.Executor.__init__.__code__):
        if not code.co_filename.startswith("<"):
            fragments.append(code.co_filename)
    return tuple(fragment for fragment in fragments if fragment)


def _in_pytest(path: str) -> bool:
    """Whether ``path`` is pytest's own code (pytest may raise ``skip`` and
    ``fail`` in an example), when pytest is imported."""
    for name in ("_pytest", "pytest", "pluggy"):
        module = sys.modules.get(name)
        file = getattr(module, "__file__", None)
        if not isinstance(file, str):
            continue
        directory = file.rpartition(os.sep)[0]
        if directory and path.startswith(directory + os.sep):
            return True
    return False


def _qualified(exc: BaseException) -> set[str]:
    return {aye_engine.qualified_name(t) for t in type(exc).__mro__}


def _pytest_outcome(exc: BaseException) -> str | None:
    """``skip``, ``xfail`` or ``fail`` for pytest's outcome exceptions, which
    its doctest runner lets end the test rather than fail the example.
    pytest gives ``Skipped`` and ``Failed`` the module ``builtins``, so they
    are told by pytest's own classes, imported when they can be raised."""
    outcomes = sys.modules.get("_pytest.outcomes")
    if outcomes is None or not isinstance(exc, outcomes.OutcomeException):
        return None
    if isinstance(exc, outcomes.Skipped):
        return "skip"
    if isinstance(exc, outcomes.XFailed):
        return "xfail"
    return "fail"


def _ran_ids(context: Any) -> Counter[str]:
    """The context's ``ran_ids``, counted."""
    found = context.get("ran_ids") if isinstance(context, dict) else None
    if not isinstance(found, list):
        return Counter()
    return Counter(item for item in found if isinstance(item, str))


def without_junitxml(args: Sequence[str]) -> list[str]:
    """``args`` without ``--junitxml``/``--junit-xml`` and their value."""
    kept: list[str] = []
    rest = iter(args)
    for arg in rest:
        if arg in JUNITXML:
            next(rest, None)
            continue
        option, equals, _ = arg.partition("=")
        if equals and option in JUNITXML:
            continue
        kept.append(arg)
    return kept


def _message(exc: BaseException) -> str:
    msg = getattr(exc, "msg", None)
    return str(msg) if msg is not None else str(exc)


# ------------------------------------------------------------------ settings


def optionflag(name: str) -> int:
    """A doctest option flag by name: a registered one (pytest's three
    included), or one of doctest's combinations (``COMPARISON_FLAGS``,
    ``REPORTING_FLAGS``), which pytest accepts too."""
    flag = doctest.OPTIONFLAGS_BY_NAME.get(name)
    if flag is None:
        flag = getattr(doctest, name, None) if name.endswith("_FLAGS") else None
        if not isinstance(flag, int):
            raise KeyError(name)
    return flag


class Settings:
    """The recognizer's context: what pytest's configuration says about
    doctests."""

    def __init__(self, context: Any, root: str) -> None:
        context = context if isinstance(context, dict) else {}
        self.rootdir = _path.abspath(context.get("rootdir") or root)
        names = context.get("optionflags")
        if names is None:
            names = ["ELLIPSIS"]
        self.optionflags = 0
        for name in names:
            self.optionflags |= optionflag(name)
        self.encoding = context.get("encoding") or "utf-8"
        self.import_mode = context.get("import_mode") or "prepend"
        self.namespace_packages = bool(context.get("consider_namespace_packages"))
        self.pythonpath = [str(p) for p in context.get("pythonpath") or ()]
        self.report = REPORTS.get(str(context.get("report") or "udiff").lower(), 0)
        self.ignore_import_errors = bool(context.get("ignore_import_errors"))
        # ``--doctest-continue-on-failure``: a later skip or xfail then ends
        # a test whose examples failed, as in pytest.
        self.continue_on_failure = bool(context.get("continue_on_failure"))
        # pytest's ``filterwarnings`` and ``-W``/``--pythonwarnings``.
        self.filterwarnings = [str(f) for f in context.get("filterwarnings") or ()]
        self.pythonwarnings = [str(f) for f in context.get("pythonwarnings") or ()]
        # The conftests pytest loads before collecting (in order), and
        # where it stops looking upwards for more; ``None`` without them
        # (``--noconftest``, or a context that doesn't say).
        conftests = context.get("conftests")
        self.conftests: dict[str, Any] | None = conftests if isinstance(conftests, dict) else None
        # When the command line selects by more than paths (a dotted name, a
        # test key), the binary names the paths the user's run starts from
        # (``selected_paths``: the path arguments, and the files the other
        # selectors resolve to): pytest's initial conftests are theirs, as
        # ``pytest <those paths>`` loads them, not those of its default the
        # recognizer worked out without paths.
        selected = context.get("selected_paths")
        if self.conftests is not None and isinstance(selected, list):
            initial = initial_conftests(
                [str(path) for path in selected if isinstance(path, str)],
                self.conftests.get("confcutdir"),
            )
            if initial is not None:
                self.conftests = {**self.conftests, "initial": initial}

    def extend_path(self) -> None:
        """``pythonpath`` first on ``sys.path``, in order, as pytest puts it."""
        for path in reversed(self.pythonpath):
            if path not in sys.path:
                sys.path.insert(0, path)


# ------------------------------------------------------------------ pytest's session

_ACTIONS = ("default", "always", "all", "ignore", "module", "once", "error")


def _warning_filter(arg: str, *, escape: bool) -> tuple[Any, ...] | None:
    """pytest's ``parse_warning_filter``: ``action:message:category:module:line``
    as ``warnings.filterwarnings`` takes it, or ``None`` when pytest would
    refuse it."""
    parts = arg.split(":")
    if len(parts) > 5:  # noqa: PLR2004 - warnings' five fields
        return None
    parts += [""] * (5 - len(parts))
    action, message, category_name, module, line = (part.strip() for part in parts)
    if not action:
        action = "default"
    action = next((a for a in _ACTIONS if a.startswith(action)), None)
    if action is None or (action == "all" and sys.version_info < (3, 14)):
        return None
    try:
        if not category_name:
            category: Any = Warning
        elif "." not in category_name:
            category = getattr(builtins, category_name, None)
        else:
            owner, _, name = category_name.rpartition(".")
            category = getattr(importlib.import_module(owner), name)
        if not (isinstance(category, type) and issubclass(category, Warning)):
            return None
        lineno = int(line) if line else 0
        if lineno < 0:
            return None
        if escape:
            message = re.escape(message) if message else message
            module = re.escape(module) + r"\Z" if module else module
        re.compile(message)
        re.compile(module)
    except Exception:  # noqa: BLE001 - pytest warns or refuses; aye skips the filter
        return None
    return action, message, category, module, lineno


@contextmanager
def pytest_warnings(settings: Settings) -> Iterator[None]:
    """pytest's warning filters, as ``catch_warnings_for_item`` sets them
    around collection and each test: deprecation warnings shown when neither
    ``-W`` nor ``PYTHONWARNINGS`` configures any, then ``filterwarnings``,
    then ``-W`` (``--pythonwarnings``), which win. Warnings still go where
    Python sends them (the test's captured ``stderr``)."""
    with warnings.catch_warnings():
        if not sys.warnoptions:
            warnings.filterwarnings("always", category=DeprecationWarning)
            warnings.filterwarnings("always", category=PendingDeprecationWarning)
        for arg in settings.filterwarnings:
            parsed = _warning_filter(arg, escape=False)
            if parsed is not None:
                warnings.filterwarnings(*parsed)
        for arg in settings.pythonwarnings:
            parsed = _warning_filter(arg, escape=True)
            if parsed is not None:
                warnings.filterwarnings(*parsed)
        yield


def pytest_version() -> str | None:
    """The version of the pytest this interpreter would import, found
    without importing it; ``None`` without one."""
    try:
        from importlib.metadata import version  # noqa: PLC0415 - only once, when needed
        from importlib.util import find_spec  # noqa: PLC0415 - see above

        if find_spec("pytest") is None:
            return None
        return version("pytest")
    except Exception:  # noqa: BLE001 - no pytest
        return None


# ------------------------------------------------------------------ importing


class ImportPathMismatchError(ImportError):
    """The module imported under a file's name is another file."""


class CollectError(Exception):
    """A file pytest would fail to collect, with pytest's message."""


class ModuleSkipped(Exception):
    """A file pytest would skip while collecting it."""

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


class ConftestSkipped(Exception):
    """A conftest raised pytest's ``skip`` while pytest loaded it for a
    directory: pytest skips the directory, so the files below it."""

    def __init__(self, path: str, reason: str) -> None:
        super().__init__(reason)
        self.path = path  # its directory's, relative to the root
        self.reason = reason


class ConftestFailed(Exception):
    """A conftest failed to load: pytest collects nothing below it. ``code``
    is ``usage-error`` for one it loads at start-up (pytest exits 4, or
    stops on the exception), else ``collection-error`` (its directory's)."""

    def __init__(self, path: str, code: str, message: str) -> None:
        super().__init__(message)
        self.path = path  # the conftest's, relative to the root
        self.code = code
        self.message = message


def _identifier(name: str) -> bool:
    return name.isidentifier()


def package_path(path: str) -> str | None:
    """pytest's ``resolve_package_path``: the last directory upward that
    still holds an ``__init__.py`` and has an identifier for a name."""
    result = None
    parent = _path.dirname(path)
    while True:
        if not _path.isfile(_path.join(parent, "__init__.py")):
            break
        if not _identifier(_path.basename(parent)):
            break
        result = parent
        up = _path.dirname(parent)
        if up == parent:
            break
        parent = up
    return result


def module_name(path: str) -> tuple[str, str]:
    """The directory pytest's ``prepend`` mode puts on ``sys.path`` for
    ``path``, and the name it imports it under."""
    package = package_path(path)
    if package is not None:
        root = _path.dirname(package)
        rel = _path.relpath(_path.splitext(path)[0], root)
        names = rel.split(os.sep)
        if names[-1] == "__init__":
            names.pop()
        if names:
            return root, ".".join(names)
    return _path.dirname(path), _path.splitext(_path.basename(path))[0]


_UNKNOWN = object()


def _parents(path: str) -> list[str]:
    """``path``'s parent directories, nearest first."""
    out = []
    while True:
        parent = _path.dirname(path)
        if parent == path:
            return out
        out.append(parent)
        path = parent


def initial_conftests(paths: Sequence[str], confcutdir: Any) -> list[str] | None:
    """pytest's ``_set_initial_conftests`` for the absolute ``paths`` it is
    given, in the order it imports them: for each path that exists, and
    each ``test*`` directory of one that is a directory (by name), the
    conftests of its directory and of those above it, from the top down,
    but those above ``confcutdir``. ``None`` when no path exists: pytest
    then starts from the current directory, as the context's ``initial``
    says."""
    above: set[str] = set()
    if isinstance(confcutdir, str) and confcutdir:
        above.update(_parents(_path.normpath(confcutdir)))
    anchors: list[str] = []
    for given in paths:
        path = _path.normpath(given)
        if _path.isdir(path):
            anchors.append(path)
            try:
                names = sorted(_listdir(path))
            except OSError:
                names = []
            anchors.extend(
                _path.join(path, name)
                for name in names
                if name.startswith("test") and _path.isdir(_path.join(path, name))
            )
        elif _path.exists(path):
            anchors.append(_path.dirname(path))
    if not anchors:
        return None
    out: list[str] = []
    seen: set[str] = set()
    for directory in dict.fromkeys(anchors):
        for parent in reversed([directory, *_parents(directory)]):
            candidate = _path.join(parent, "conftest.py")
            if parent in above or candidate in seen:
                continue
            seen.add(candidate)
            if _path.isfile(candidate):
                out.append(candidate)
    return out


def _same_file(a: str, b: str) -> bool:
    try:
        return _path.samefile(a, b)
    except (OSError, ValueError):
        return False


def _forget_other_conftest(path: str, name: str) -> None:
    """Every ``conftest.py`` outside a package is ``conftest``: pytest loads
    each one afresh (``_importconftest``) before collecting its directory,
    so none is another's "import file mismatch"."""
    if _path.basename(path) != "conftest.py" or package_path(path) is not None:
        return
    existing = getattr(sys.modules.get(name), "__file__", None)
    if isinstance(existing, str) and not _same_file(existing, path):
        del sys.modules[name]


def import_like_pytest(path: str, settings: Settings) -> Any:
    """Import ``path`` the way pytest's ``import_path`` does for the
    configured import mode."""
    if settings.import_mode == "importlib" or settings.namespace_packages:
        try:
            from _pytest.pathlib import import_path  # noqa: PLC0415 - only this mode needs it
        except ImportError:  # pytest isn't installed: its default mode
            pass
        else:
            return import_path(
                path,
                mode=settings.import_mode,
                root=Path(settings.rootdir),
                consider_namespace_packages=settings.namespace_packages,
            )
    root, name = module_name(path)
    _forget_other_conftest(path, name)
    if settings.import_mode == "append":
        if root not in sys.path:
            sys.path.append(root)
    elif not sys.path or sys.path[0] != root:
        sys.path.insert(0, root)
    importlib.import_module(name)
    module = sys.modules[name]
    if _path.basename(path) == "__init__.py":
        return module
    if os.environ.get("PY_IGNORE_IMPORTMISMATCH", "") != "1":
        module_file = getattr(module, "__file__", None)
        if module_file is None:
            raise ImportPathMismatchError(name, module_file, path)
        if module_file.endswith((".pyc", ".pyo")):
            module_file = module_file[:-1]
        module_file = module_file.removesuffix(os.sep + "__init__.py")
        if not _same_file(path, module_file):
            raise ImportPathMismatchError(name, module_file, path)
    return module


def import_conftest(path: str, settings: Settings) -> Any:
    """pytest's ``_importconftest``: a conftest outside a package is
    ``conftest``, whichever was imported under that name before."""
    if package_path(path) is None:
        sys.modules.pop("conftest", None)
    return import_like_pytest(path, settings)


def _short_traceback(exc: BaseException, paths: aye_engine.Paths) -> str:
    """The traceback without the import machinery's frames, as pytest shows
    an import error."""
    frames = [
        frame
        for frame in extract_tb(exc.__traceback__)
        if not paths.is_internal(frame.filename)
        and not frame.filename.startswith("<frozen")
        and "importlib" not in frame.filename.replace(os.sep, "/").split("/")
    ]
    text = "".join(format_list(frames))
    return text + "".join(format_exception_only(type(exc), exc))


def import_module(path: str, settings: Settings, paths: aye_engine.Paths) -> Any:
    """``importtestmodule``: the module, or ``CollectError`` /
    ``ModuleSkipped`` with pytest's message."""
    try:
        return import_like_pytest(path, settings)
    except SyntaxError as exc:
        raise CollectError("".join(format_exception_only(type(exc), exc))) from exc
    except ImportPathMismatchError as exc:
        name, file, wanted = exc.args
        raise CollectError(
            "import file mismatch:\n"
            f"imported module {name!r} has this __file__ attribute:\n"
            f"  {file}\n"
            "which is not the same as the test file we want to collect:\n"
            f"  {wanted}\n"
            "HINT: remove __pycache__ / .pyc files and/or use a unique basename "
            "for your test file modules"
        ) from exc
    except ImportError as exc:
        if type(exc).__name__ == "ImportPathMismatchError":  # pytest's own
            raise CollectError(f"import file mismatch:\n{exc}") from exc
        raise CollectError(
            f"ImportError while importing test module '{path}'.\n"
            "Hint: make sure your test modules/packages have valid Python names.\n"
            "Traceback:\n" + _short_traceback(exc, paths)
        ) from exc
    except KeyboardInterrupt:
        raise
    except BaseException as exc:
        names = _qualified(exc)
        if _pytest_outcome(exc) == "skip":
            if getattr(exc, "allow_module_level", False):
                raise ModuleSkipped(_message(exc)) from exc
            raise CollectError(
                "Using pytest.skip outside of a test will skip the entire module. "
                "If that's your intention, pass `allow_module_level=True`. "
                "If you want to skip a specific test or an entire class, "
                "use the @pytest.mark.skip or @pytest.mark.skipif decorators."
            ) from exc
        if "unittest.case.SkipTest" in names:
            raise ModuleSkipped(str(exc)) from exc
        raise CollectError(_short_traceback(exc, paths)) from exc


# ------------------------------------------------------------------ finding


_LINENO_BROKEN = (
    sys.version_info[:2] < (3, 11)
    or (sys.version_info[:2] == (3, 11) and sys.version_info.micro < 9)
    or (sys.version_info[:2] == (3, 12) and sys.version_info.micro < 3)
)


class _Finder(doctest.DocTestFinder):
    """``DocTestFinder`` with pytest's adjustments."""

    if _LINENO_BROKEN:

        def _find_lineno(self, obj: Any, source_lines: Any) -> Any:
            """Before 3.11.9 and 3.12.3, doctest misses the line of a
            property and of a wrapped function."""
            if isinstance(obj, property):
                obj = getattr(obj, "fget", obj)
            if hasattr(obj, "__wrapped__"):
                obj = inspect.unwrap(obj)
            return super()._find_lineno(obj, source_lines)  # type: ignore[misc]

    if sys.version_info < (3, 13):

        def _from_module(self, module: Any, object: Any) -> Any:
            """Before 3.13, doctest skips ``cached_property`` objects."""
            if isinstance(object, functools.cached_property):
                object = object.func
            return super()._from_module(module, object)  # type: ignore[misc]


class Loaded:
    """One file's tests, or why it has none."""

    def __init__(self, module_name: str | None = None) -> None:
        self.tests: dict[str, doctest.DocTest] = {}
        self.module_name = module_name
        self.error: str | None = None
        self.skipped: str | None = None


def find_in_module(module: Any) -> dict[str, doctest.DocTest]:
    tests = _Finder().find(module, module.__name__)
    return {test.name: test for test in tests if test.examples}


def find_in_text(path: str, settings: Settings) -> dict[str, doctest.DocTest]:
    text = Path(path).read_text(settings.encoding)
    name = _path.basename(path)
    test = doctest.DocTestParser().get_doctest(text, {"__name__": "__main__"}, name, path, 0)
    return {test.name: test} if test.examples else {}


# ------------------------------------------------------------------ checking


class Checker(doctest.OutputChecker):
    """pytest's ``LiteralsOutputChecker``: ``ALLOW_UNICODE`` and
    ``ALLOW_BYTES`` ignore ``u''`` and ``b''`` prefixes, and ``NUMBER``
    ignores digits beyond those the expected output shows."""

    _unicode_literal_re = re.compile(r"(\W|^)[uU]([rR]?[\'\"])", re.UNICODE)
    _bytes_literal_re = re.compile(r"(\W|^)[bB]([rR]?[\'\"])", re.UNICODE)
    _number_re = re.compile(
        r"""
        (?P<number>
          (?P<mantissa>
            (?P<integer1> [+-]?\d*)\.(?P<fraction>\d+)
            |
            (?P<integer2> [+-]?\d+)\.
          )
          (?:
            [Ee]
            (?P<exponent1> [+-]?\d+)
          )?
          |
          (?P<integer3> [+-]?\d+)
          (?:
            [Ee]
            (?P<exponent2> [+-]?\d+)
          )
        )
        """,
        re.VERBOSE,
    )

    def check_output(self, want: str, got: str, optionflags: int) -> bool:
        if super().check_output(want, got, optionflags):
            return True
        allow_unicode = optionflags & ALLOW_UNICODE
        allow_bytes = optionflags & ALLOW_BYTES
        allow_number = optionflags & NUMBER
        if not allow_unicode and not allow_bytes and not allow_number:
            return False

        def remove_prefixes(regex: re.Pattern[str], text: str) -> str:
            return re.sub(regex, r"\1\2", text)

        if allow_unicode:
            want = remove_prefixes(self._unicode_literal_re, want)
            got = remove_prefixes(self._unicode_literal_re, got)
        if allow_bytes:
            want = remove_prefixes(self._bytes_literal_re, want)
            got = remove_prefixes(self._bytes_literal_re, got)
        if allow_number:
            got = self._remove_unwanted_precision(want, got)
        return super().check_output(want, got, optionflags)

    def _remove_unwanted_precision(self, want: str, got: str) -> str:
        wants = list(self._number_re.finditer(want))
        gots = list(self._number_re.finditer(got))
        if len(wants) != len(gots):
            return got
        offset = 0
        for w, g in zip(wants, gots, strict=True):
            fraction: str | None = w.group("fraction")
            exponent: str | None = w.group("exponent1")
            if exponent is None:
                exponent = w.group("exponent2")
            precision = 0 if fraction is None else len(fraction)
            if exponent is not None:
                precision -= int(exponent)
            if _close(float(w.group()), float(g.group()), 10**-precision):
                got = got[: g.start() + offset] + w.group() + got[g.end() + offset :]
                offset += w.end() - w.start() - (g.end() - g.start())
        return got


def _close(wanted: float, got: float, tolerance: float) -> bool:
    """``wanted == pytest.approx(got, abs=tolerance)``."""
    if wanted == got:
        return True
    if math.isnan(got) or math.isinf(got):
        return False
    return abs(got - wanted) <= tolerance


# ------------------------------------------------------------------ running


class _ExamplesFailed(AssertionError):
    """Some examples failed; their failures, in order."""

    def __init__(self, failures: list[Failure]) -> None:
        super().__init__(f"{len(failures)} example(s) failed")
        self.failures = failures


def _effective_flags(base: int, example: doctest.Example) -> int:
    """The flags doctest runs ``example`` with: the test's, changed by the
    example's own directives."""
    flags = base
    for flag, on in (example.options or {}).items():
        flags = flags | flag if on else flags & ~flag
    return flags


def _exact(flags: int, example: doctest.Example) -> bool:
    """Whether an example's output had to be what it expects, character for
    character (blank lines aside), so that where the two first differ is
    where it failed: no flag loosened the match for what it expects (an
    ellipsis, an exception's detail)."""
    if flags & _LOOSE_MATCHING:
        return False
    if flags & doctest.ELLIPSIS and doctest.ELLIPSIS_MARKER in example.want:
        return False
    return not (flags & doctest.IGNORE_EXCEPTION_DETAIL and example.exc_msg is not None)


def _first_line(text: str) -> str:
    lines = text.splitlines()
    return lines[0] if lines else ""


def _shown(text: str) -> str:
    """Output as a failure message shows it: its repr, without the final
    newline; ``nothing`` when empty."""
    if not text:
        return "nothing"
    return repr(text.removesuffix("\n"))


def _without_newline(text: str) -> str:
    return text.removesuffix("\n")


class _Examples:
    """Reports one ``DocTest``'s examples as they run: a ``sub_outcome``
    each, and the failures."""

    def __init__(
        self,
        executor: DoctestExecutor,
        test_id: str,
        dtest: doctest.DocTest,
        sink: Sink,
        flags: int,
    ) -> None:
        self.executor = executor
        self.test_id = test_id
        self.dtest = dtest
        self.sink = sink
        self.flags = flags
        self.index = {id(example): at for at, example in enumerate(dtest.examples)}
        self.reported = 0
        self.started = perf_counter()
        self.failures: list[Failure] = []
        self.path = executor.paths.relative(dtest.filename) if dtest.filename else None

    def line(self, example: doctest.Example) -> int | None:
        if self.dtest.lineno is None:
            return None
        return self.dtest.lineno + example.lineno + 1

    def name(self, example: doctest.Example) -> str:
        source = f">>> {_first_line(example.source)}"
        line = self.line(example)
        return f"line {line}: {source}" if line is not None else source

    def emit(
        self, example: doctest.Example, outcome: Outcome, failure: Failure | None = None
    ) -> None:
        duration = perf_counter() - self.started
        self.sink.sub_outcome(self.test_id, self.name(example), outcome, failure, duration)
        self.reported = self.index[id(example)] + 1

    def skip_until(self, at: int) -> None:
        """Examples doctest passed over before ``at`` had ``SKIP``."""
        for example in self.dtest.examples[self.reported : at]:
            self.started = perf_counter()
            self.emit(example, Outcome.SKIPPED)

    def start(self, example: doctest.Example) -> None:
        self.skip_until(self.index[id(example)])
        self.started = perf_counter()

    def success(self, example: doctest.Example) -> None:
        self.emit(example, Outcome.PASSED)

    def site(self, example: doctest.Example) -> str:
        """Where an example is, for the signature of a failure doctest can
        give no line for (a ``__test__`` string, an object it can't place):
        the test and the example's position in it."""
        return f"{self.test_id}[{self.index.get(id(example), -1)}]"

    def mismatch(self, example: doctest.Example, got: str) -> None:
        try:
            failure = self.mismatch_failure(example, got)
        except Exception:  # noqa: BLE001 - what an example broke mustn't lose it
            source = _first_line(example.source)
            failure = Failure(
                f"{source}: expected {_shown(example.want)}, got {_shown(got)}",
                phase=Phase.TEST,
                exception="DocTestFailure",
                location=self.location(example),
            )
        failure.site = self.site(example)
        self.failures.append(failure)
        self.emit(example, Outcome.FAILED, failure)

    def unexpected(self, example: doctest.Example, exc: BaseException) -> None:
        try:
            failure = self.exception_failure(example, exc)
        except Exception:  # noqa: BLE001 - what an example broke mustn't lose it
            failure = Failure.from_exception(exc, self.executor.paths, phase=Phase.TEST)
        failure.site = self.site(example)
        self.failures.append(failure)
        self.emit(example, Outcome.FAILED, failure)

    def outcome(self, example: doctest.Example, exc: BaseException) -> Failure | None:
        """pytest's ``skip``, ``xfail`` or ``fail`` ends the test here."""
        kind = _pytest_outcome(exc)
        if kind == "skip":
            self.emit(example, Outcome.SKIPPED)
            return None
        if kind == "xfail":
            self.emit(example, Outcome.EXPECTED_FAILURE)
            return None
        failure = self.exception_failure(example, exc)
        failure.site = self.site(example)
        self.emit(example, Outcome.FAILED, failure)
        return failure

    def finish(self) -> None:
        """The examples after the last one reported: skipped, up to the
        first that wasn't (``FAIL_FAST`` stopped the run there)."""
        for example in self.dtest.examples[self.reported :]:
            if not _effective_flags(self.flags, example) & doctest.SKIP:
                break
            self.started = perf_counter()
            self.emit(example, Outcome.SKIPPED)

    # -------------------------------------------------------------- failures

    def header(self, example: doctest.Example) -> str:
        line = self.line(example)
        where = f'File "{self.path}", line {line}' if line is not None else f'File "{self.path}"'
        source = "".join(f"    {text}\n" for text in example.source.splitlines())
        return f"{where}, in {self.dtest.name}\nFailed example:\n{source}"

    def location(self, example: doctest.Example) -> tuple[str, int] | None:
        line = self.line(example)
        if self.path is None or line is None:
            return None
        return (self.path, line)

    def mismatch_failure(self, example: doctest.Example, got: str) -> Failure:
        want = example.want
        source = _first_line(example.source)
        location = self.location(example)
        frames = []
        if location is not None:
            frames.append(
                {
                    "path": location[0],
                    "line": location[1],
                    "function": self.dtest.name,
                    "source": f">>> {source}",
                }
            )
        shown = got
        if not _effective_flags(self.flags, example) & doctest.DONT_ACCEPT_BLANKLINE:
            # As ``output_difference`` shows it: an empty line of output is
            # what ``<BLANKLINE>`` expects, not a difference.
            shown = re.sub(r"(?m)^[ ]*(?=\n)", doctest.BLANKLINE_MARKER, got)
        diff = list(difflib.unified_diff(want.splitlines(), shown.splitlines(), n=2, lineterm=""))[
            2:
        ]
        checker = self.executor.checker
        report = self.executor.settings.report
        detail = self.header(example) + checker.output_difference(example, got, report)
        assertion: dict[str, Any] = {
            "op": "==",
            "left": _without_newline(got),
            "right": _without_newline(want),
            "diff": diff,
            "expected": "right",
            # What printed the output: the example's code.
            "left_source": source,
            "right_source": "expected output",
        }
        if _exact(_effective_flags(self.flags, example), example):
            # Blank lines as the diff shows them.
            compared = aye_engine.compare(_without_newline(shown), assertion["right"], "right")
            if compared is not None:
                assertion["compare"] = compared
        return Failure(
            f"{source}: expected {_shown(want)}, got {_shown(got)}",
            phase=Phase.TEST,
            exception="DocTestFailure",
            assertion=assertion,
            location=location,
            frames=frames,
            detail=detail,
        )

    def exception_failure(self, example: doctest.Example, exc: BaseException) -> Failure:
        frames = self.frames(exc)
        location = (frames[-1]["path"], frames[-1]["line"]) if frames else self.location(example)
        paths = self.executor.paths
        stack = [
            summary
            for summary in extract_tb(exc.__traceback__)
            if not paths.is_internal(summary.filename) and not _in_pytest(summary.filename)
        ]
        text = "Traceback (most recent call last):\n" + "".join(format_list(stack))
        text += "".join(format_exception_only(type(exc), exc))
        detail = (
            self.header(example)
            + "Exception raised:\n"
            + "".join(f"    {line}\n" for line in text.splitlines())
        )
        return Failure(
            aye_engine.exception_message(exc),
            phase=Phase.TEST,
            exception=type(exc).__name__,
            location=location,
            frames=frames,
            detail=detail,
        )

    def frames(self, exc: BaseException) -> list[dict[str, Any]]:
        """The user's frames, an example's own mapped to its file and line."""
        paths = self.executor.paths
        out: list[dict[str, Any]] = []
        for summary in extract_tb(exc.__traceback__):
            match = _DOCTEST_FRAME.match(summary.filename)
            if match and match.group("name") == self.dtest.name:
                at = int(match.group("index"))
                if at >= len(self.dtest.examples) or self.dtest.lineno is None:
                    continue
                example = self.dtest.examples[at]
                lineno = summary.lineno or 1
                source_lines = example.source.splitlines()
                frame: dict[str, Any] = {
                    "path": self.path,
                    "line": self.dtest.lineno + example.lineno + lineno,
                    "function": self.dtest.name,
                }
                if 0 < lineno <= len(source_lines):
                    prompt = ">>>" if lineno == 1 else "..."
                    frame["source"] = f"{prompt} {source_lines[lineno - 1]}".rstrip()
                out.append(frame)
                continue
            if (
                paths.is_internal(summary.filename)
                or summary.filename.startswith("<")
                or _in_pytest(summary.filename)
            ):
                continue
            frame = {
                "path": paths.relative(summary.filename),
                "line": summary.lineno or 0,
                "function": summary.name,
            }
            source = (summary.line or "").strip()
            if source:
                frame["source"] = source
            out.append(frame)
        return out


class _Runner(doctest.DocTestRunner):
    """A ``DocTestRunner`` that reports every example through
    ``_Examples`` and lets pytest's outcome exceptions end the test, as
    pytest's runner does. ``REPORT_ONLY_FIRST_FAILURE`` only quiets
    doctest's own reports, and would quiet these hooks too: it is kept out
    of the flags doctest reads."""

    def __init__(self, checker: doctest.OutputChecker, optionflags: int) -> None:
        self._flags = 0
        self.examples: _Examples | None = None
        self.ended_by: tuple[BaseException, Failure | None] | None = None
        super().__init__(checker=checker, verbose=False, optionflags=optionflags)

    @property  # type: ignore[override]
    def optionflags(self) -> int:
        return self._flags & ~doctest.REPORT_ONLY_FIRST_FAILURE

    @optionflags.setter
    def optionflags(self, value: int) -> None:
        self._flags = value

    def report_start(self, out: Any, test: Any, example: Any) -> None:
        if self.examples is not None:
            self.examples.start(example)

    def report_success(self, out: Any, test: Any, example: Any, got: str) -> None:
        if self.examples is not None:
            self.examples.success(example)

    def report_failure(self, out: Any, test: Any, example: Any, got: str) -> None:
        if self.examples is not None:
            self.examples.mismatch(example, got)

    def report_unexpected_exception(self, out: Any, test: Any, example: Any, exc_info: Any) -> None:
        exc = exc_info[1]
        if _pytest_outcome(exc) is not None:
            failure = self.examples.outcome(example, exc) if self.examples is not None else None
            self.ended_by = (exc, failure)
            raise exc
        if self.examples is not None:
            self.examples.unexpected(example, exc)


class DoctestTest(aye_engine.Test):
    """One ``DocTest``."""

    def __init__(self, test_id: str, dtest: doctest.DocTest, executor: DoctestExecutor) -> None:
        super().__init__(test_id)
        self.dtest = dtest
        self.executor = executor
        # The pytest outcome exception that ended the test in an example, and
        # the failure that example reported for it.
        self.ended_by: tuple[BaseException, Failure | None] | None = None
        # The examples that failed before it ended.
        self.failures: list[Failure] = []

    def body(self) -> None:
        self.executor.run_doctest(self)


class _Capture:
    """Capture for doctests when the runner provides none: ``stderr``, and
    ``stdout`` written outside examples (doctest takes ``stdout`` itself
    while they run), through ``aye_engine.captured``: streams with
    ``.buffer``, ``.encoding`` and ``fileno()``, read even when an example
    closed them."""

    def capture(self, result: Result) -> Any:
        return aye_engine.captured(result)


# ------------------------------------------------------------------ the executor


class DoctestExecutor(aye_engine.EngineExecutor):
    """Runs batches of doctests and refines files into their doctests."""

    adapter = "doctest"
    internal = _internal_fragments()

    def __init__(self, root: str, services: Services) -> None:
        super().__init__(root, services)
        self.settings = Settings(None, self.root)
        self.checker = Checker()
        self.sink: Sink | None = None
        self.started: list[str] = []
        self.collection_failed = False
        # Conftests imported, by path: ``None``, or why one failed or
        # skipped (``(code, relative path, text)``, ``code`` ``skip`` for
        # pytest's skip).
        self.conftests: dict[str, tuple[str, str, str] | None] = {}
        # The diagnostics batches on this worker have written: a broken
        # conftest is one error in pytest, however many files below it a
        # batch loads.
        self.reported: set[tuple[str, str, str, str]] = set()
        self._pytest_version: object = _UNKNOWN

    # -------------------------------------------------------------- files

    def absolute(self, path: str) -> str:
        """A project-relative path (``/`` separators), made absolute."""
        return _path.normpath(_path.join(self.root, path))

    def load_file(self, file: str, diagnose: Any) -> Loaded:
        """Import or read ``file`` and find its doctests, as pytest collects
        it: after the conftests pytest has loaded by then, under its warning
        filters, with ``os.path`` the standard library's own (pytest collects
        before any test runs). Problems go to ``diagnose(severity, code,
        path, message)``."""
        path = self.absolute(file)
        if not _path.exists(path):
            diagnose("error", "not-found", file, f"{file}: no such file")
            loaded = Loaded()
            loaded.error = "missing"
            return loaded
        with aye_engine.unpatched_os_path(), pytest_warnings(self.settings):
            return self._load(file, path, diagnose)

    def _load(self, file: str, path: str, diagnose: Any) -> Loaded:
        try:
            self.load_conftests(path)
        except ConftestSkipped as exc:
            # The conftest's one skip, as pytest reports its directory's.
            loaded = Loaded()
            loaded.skipped = exc.reason
            diagnose("info", "skipped", exc.path, exc.reason)
            return loaded
        except ConftestFailed as exc:
            loaded = Loaded()
            loaded.error = exc.message
            diagnose("error", exc.code, exc.path, exc.message)
            return loaded
        if not path.endswith(".py"):
            loaded = Loaded()
            try:
                loaded.tests = find_in_text(path, self.settings)
            except (OSError, UnicodeError, ValueError, LookupError) as exc:
                loaded.error = f"{type(exc).__name__}: {exc}"
                diagnose("error", "collection-error", file, loaded.error)
            return loaded
        try:
            module = import_module(path, self.settings, self.paths)
        except (CollectError, ModuleSkipped) as exc:
            loaded = Loaded()
            if isinstance(exc, ModuleSkipped):
                loaded.skipped = exc.reason
            elif self.settings.ignore_import_errors:
                loaded.skipped = f"unable to import module {Path(path)!r}"
            else:
                loaded.error = str(exc)
                diagnose("error", "collection-error", file, loaded.error)
            if loaded.skipped is not None:
                diagnose("info", "skipped", file, loaded.skipped)
            return loaded
        loaded = Loaded(getattr(module, "__name__", None))
        try:
            loaded.tests = find_in_module(module)
        except KeyboardInterrupt:
            raise
        except BaseException as exc:  # noqa: BLE001 - pytest reports it as the file's error
            loaded.error = _short_traceback(exc, self.paths)
            diagnose("error", "collection-error", file, loaded.error)
        return loaded

    # -------------------------------------------------------------- pytest's session

    def load_conftests(self, path: str) -> None:
        """Import the conftests pytest has loaded when it collects ``path``
        (when pytest is installed: they are its plugins): the initial ones
        the context lists, which pytest loads at start-up, then each
        directory's from the top down to ``path``'s, as pytest loads them
        for each directory it collects (``_loadconftestmodules``), but those
        above ``confcutdir``; each once, as pytest's ``_importconftest``,
        and none below one that failed or skipped, as pytest collects
        nothing there. Raises ``ConftestSkipped`` for one that raised
        pytest's ``skip`` (pytest skips its directory), ``ConftestFailed``
        for one that failed (or an initial one that skipped, which stops
        pytest before its session)."""
        conftests = self.settings.conftests
        if conftests is None or not self.pytest_version():
            return
        for conftest in conftests.get("initial") or ():
            self._conftest(_path.normpath(str(conftest)), initial=True)
        above: set[str] = set()
        cut = conftests.get("confcutdir")
        if isinstance(cut, str) and cut:
            above.update(_parents(_path.normpath(cut)))
        for directory in reversed([_path.dirname(path), *_parents(_path.dirname(path))]):
            if directory in above:
                continue
            candidate = _path.join(directory, "conftest.py")
            if _path.isfile(candidate):
                self._conftest(candidate, initial=False)

    def _conftest(self, path: str, *, initial: bool) -> None:
        if path not in self.conftests:
            self.conftests[path] = None
            try:
                import_conftest(path, self.settings)
            except KeyboardInterrupt:
                raise
            except BaseException as exc:  # noqa: BLE001 - the files below report it
                self.conftests[path] = self._conftest_problem(path, exc, initial=initial)
        problem = self.conftests[path]
        if problem is None:
            return
        code, rel, text = problem
        if code == "skip":
            raise ConftestSkipped(rel, text)
        raise ConftestFailed(rel, code, text)

    def _conftest_problem(
        self, path: str, exc: BaseException, *, initial: bool
    ) -> tuple[str, str, str]:
        """What a conftest that raised ``exc`` while loading does to the
        files below it, as ``(code, relative path, text)``: pytest's
        ``skip`` skips the directory it was loaded for (the directory's
        path, the reason); an initial conftest's anything, and any other
        exception, fails it (``usage-error`` for an initial one: pytest
        exits 4, or stops on the exception, before its session; else
        ``collection-error``), on the conftest's path, with the message the
        pytest adapter gives it, word for word (renderers show an error two
        adapters report alike once): ``Skipped while loading conftest`` for
        pytest's ``skip`` in an initial one, ``Failed …`` for its ``fail``
        or ``xfail``, else ``ImportError …``, as pytest words an exception
        it wraps in its ``ConftestImportFailure``."""
        rel = self.paths.relative(path)
        outcome = _pytest_outcome(exc)
        if not initial and outcome == "skip":
            # On the directory, as pytest (and the pytest adapter) reports
            # its one skip.
            return "skip", self.paths.relative(_path.dirname(path)), _message(exc)
        kind = {"skip": "Skipped", None: "ImportError"}.get(outcome, "Failed")
        lines = [f"{kind} while loading conftest '{rel}'."]
        for frame in extract_tb(exc.__traceback__):
            if self.paths.is_internal(frame.filename) or _in_pytest(frame.filename):
                continue
            where = self.paths.relative(frame.filename)
            if "importlib" in where.split("/"):
                continue
            lines.append(f"{where}:{frame.lineno or 0}: in {frame.name}")
            source = (frame.line or getline(frame.filename, frame.lineno or 0)).strip()
            if source:
                lines.append(f"    {source}")
        lines.append(f"{type(exc).__name__}: {exc}")
        return ("usage-error" if initial else "collection-error"), rel, "\n".join(lines)

    def pytest_version(self) -> str | None:
        if self._pytest_version is _UNKNOWN:
            self._pytest_version = pytest_version()
        return self._pytest_version  # type: ignore[return-value]

    def pytest_session(self) -> None:
        """``PYTEST_VERSION``, which pytest sets for its whole session (how
        code tells it runs under pytest), when pytest is installed."""
        version = self.pytest_version()
        if version:
            os.environ["PYTEST_VERSION"] = version

    def nodeid(self, test_id: str) -> str:
        """pytest's node id for a test: its path from the rootdir."""
        file, sep, name = test_id.partition("::")
        rel = _path.relpath(self.absolute(file), self.settings.rootdir)
        if rel == os.pardir or rel.startswith(os.pardir + os.sep):
            return test_id
        return rel.replace(os.sep, "/") + sep + name

    @contextmanager
    def current(self, test: aye_engine.Test) -> Iterator[None]:
        """``PYTEST_CURRENT_TEST`` while a test's examples run, as pytest
        sets it for the call, when pytest is installed."""
        if not self.pytest_version():
            yield
            return
        try:
            os.environ["PYTEST_CURRENT_TEST"] = f"{self.nodeid(test.id)} (call)"
        except ValueError:  # a NUL in the id
            pass
        try:
            yield
        finally:
            os.environ.pop("PYTEST_CURRENT_TEST", None)

    def keywords(self, file: str, name: str) -> list[str]:
        """The names pytest's ``-k`` matches: the directories below the
        rootdir, the file name and the test's name."""
        rel = _path.relpath(self.absolute(file), self.settings.rootdir)
        if rel == os.pardir or rel.startswith(os.pardir + os.sep):
            parts = file.split("/")
        else:
            parts = rel.split(os.sep)
        if name not in parts:
            parts.append(name)
        return parts

    def case(self, file: str, loaded: Loaded, name: str, dtest: doctest.DocTest) -> dict[str, Any]:
        span: dict[str, Any] = {"path": file, "start": 0, "end": 0}
        if dtest.lineno is not None:
            span["line"] = dtest.lineno + 1
        affinity = [file]
        if loaded.module_name is not None:
            affinity.insert(0, f"import:{loaded.module_name}")
        return {
            "id": f"{file}::{name}",
            "adapter": "doctest",
            "span": span,
            "keywords": self.keywords(file, name),
            "affinity": affinity,
            "certainty": "refined",
        }

    # -------------------------------------------------------------- refine

    def refine(self, files: list[str], context: Any) -> dict[str, Any]:
        """Import (or read) ``files`` and return their doctests as cases,
        in ``DocTestFinder``'s order, and what went wrong as diagnostics."""
        self.settings = Settings(context, self.root)
        self.settings.extend_path()
        self.pytest_session()
        diagnostics: list[dict[str, Any]] = []

        def diagnose(severity: str, code: str, path: str, message: str) -> None:
            diagnostic = {
                "severity": severity,
                "code": code,
                "path": path,
                "message": aye_engine.bound(message)[0],
            }
            # A broken or skipping conftest is one diagnostic, however many
            # files below it are refined.
            if diagnostic not in diagnostics:
                diagnostics.append(diagnostic)

        cases = []
        for file in dict.fromkeys(files):
            loaded = self.load_file(file, diagnose)
            for name, dtest in loaded.tests.items():
                cases.append(self.case(file, loaded, name, dtest))
        return aye_engine.sanitize({"cases": cases, "diagnostics": diagnostics})

    # -------------------------------------------------------------- run

    def run(self, batch: aye_engine.Batch, sink: Sink) -> int | None:
        """Run ``batch``: load its files, then its doctests in its order.
        Returns 1 when a test failed or a file failed to load, else 0; 2,
        with a ``batch-stopped`` error, when a ``KeyboardInterrupt`` stopped
        it."""
        self.settings = Settings(batch.context, self.root)
        self.settings.extend_path()
        self.pytest_session()
        self.sink = sink
        self.started = []
        self.collection_failed = False
        services = self.services
        if batch.policy.capture and not services.has(Services.CAPTURE):
            provided = {name: services.get(name) for name in services.names()}
            provided[Services.CAPTURE] = _Capture()
            self.services = Services(provided)
        try:
            status = super().run(batch, sink)
        except KeyboardInterrupt:
            self._stopped(batch, "a KeyboardInterrupt stopped the batch")
            return 2
        finally:
            self.services = services
            self.sink = None
        return 1 if status or self.collection_failed else 0

    def _stopped(self, batch: aye_engine.Batch, why: str) -> None:
        started = set(self.started)
        requested = list(dict.fromkeys(batch.ids))
        unstarted = [test_id for test_id in requested if test_id not in started]
        if unstarted:
            named = ", ".join(unstarted[:MAX_NAMED])
            more = len(unstarted) - MAX_NAMED
            listed = f"{named} and {more} more" if more > 0 else named
            what = (
                f"{len(unstarted)} of {len(requested)} tests in the batch never started: {listed}"
            )
        else:
            what = "every test in the batch started"
        if self.sink is not None:
            self.sink.diagnostic("error", "batch-stopped", "", f"{why}; {what}")

    def load(self, batch: aye_engine.Batch) -> list[aye_engine.Test]:
        """The batch's tests, in its order, every file loaded first; files
        that fail to load are diagnosed and their tests left out, and so are
        the tests that ran before the worker of the batch this one retries
        crashed (the context's ``ran_ids``, by count, added by the runner:
        a file's id stands for all of its doctests)."""
        sink = self.sink
        assert sink is not None

        def diagnose(severity: str, code: str, path: str, message: str) -> None:
            # A skipped module's tests finish as skipped, with its reason.
            if severity == "info":
                return
            if severity == "error":
                self.collection_failed = True
            # Each once on this worker: a broken conftest is one error,
            # however many files below it the batches load.
            key = (severity, code, path, message)
            if key not in self.reported:
                self.reported.add(key)
                sink.diagnostic(severity, code, path, message)

        files: dict[str, Loaded] = {}
        tests: list[aye_engine.Test] = []
        for test_id in dict.fromkeys(batch.ids):
            file, sep, name = test_id.partition("::")
            if file not in files:
                files[file] = self.load_file(file, diagnose)
            loaded = files[file]
            if loaded.error is not None:
                continue  # its diagnostic covers it
            if loaded.skipped is not None:
                tests.append(aye_engine.Test(test_id, skip=loaded.skipped))
                continue
            if not sep:
                tests += [
                    DoctestTest(f"{file}::{n}", dtest, self) for n, dtest in loaded.tests.items()
                ]
                continue
            dtest = loaded.tests.get(name)
            if dtest is None:
                diagnose("error", "not-found", file, f"{test_id}: doctest found no such test")
                continue
            tests.append(DoctestTest(test_id, dtest, self))
        ran = _ran_ids(batch.context)
        kept = []
        for test in tests:
            if ran[test.id]:
                ran[test.id] -= 1
                continue
            kept.append(test)
        return kept

    def run_test(
        self, test: aye_engine.Test, blocker: aye_engine.Lifetime | None, sink: Sink
    ) -> Result:
        self.started.append(test.id)
        return super().run_test(test, blocker, sink)

    def rerun(self, test: aye_engine.Test) -> dict[str, Any] | None:
        return aye_engine.reproduce(test.id, without_junitxml(self.framework_args), self.rerun_args)

    def classify(self, exc: BaseException | None) -> Outcome:
        """pytest's verdicts for a doctest: whatever ends it early is a
        failure, but a skip."""
        if exc is None:
            return Outcome.PASSED
        if aye_engine.is_skip(exc):
            return Outcome.SKIPPED
        return Outcome.FAILED

    def _body_outcome(
        self, test: aye_engine.Test, result: Result, exc: BaseException | None
    ) -> None:
        if isinstance(exc, _ExamplesFailed):
            for failure in exc.failures:
                result.add(Outcome.FAILED, failure)
            return
        earlier = getattr(test, "failures", None) or []
        if (
            exc is not None
            and earlier
            and not self.settings.continue_on_failure
            and _pytest_outcome(exc) is not None
        ):
            # pytest stops at the first failed example (without
            # ``--doctest-continue-on-failure``): the test failed there,
            # whatever a later example would have raised.
            for failure in earlier:
                result.add(Outcome.FAILED, failure)
            ended = getattr(test, "ended_by", None)
            if ended is not None and ended[0] is exc and ended[1] is not None:
                result.add(Outcome.FAILED, ended[1])
            return
        if exc is not None and _pytest_outcome(exc) == "xfail":
            result.add(Outcome.EXPECTED_FAILURE)
            result.reason = _message(exc)
            return
        ended = getattr(test, "ended_by", None)
        if (
            exc is not None
            and ended is not None
            and ended[0] is exc
            and ended[1] is not None
            and _pytest_outcome(exc) == "fail"
        ):
            # ``pytest.fail`` in an example: its failure, as the example's.
            result.add(Outcome.FAILED, ended[1])
            return
        super()._body_outcome(test, result, exc)

    def run_doctest(self, test: DoctestTest) -> None:
        """Run one ``DocTest``, reporting each example."""
        dtest = test.dtest
        flags = self.settings.optionflags
        sink = self.sink if self.sink is not None else aye_engine.ListSink()
        examples = _Examples(self, test.id, dtest, sink, flags)
        if all(example.options.get(doctest.SKIP, False) for example in dtest.examples):
            # pytest skips the test without running any example when each
            # has its own +SKIP (``_check_all_skipped``); a SKIP among the
            # option flags only makes doctest pass over every example.
            examples.skip_until(len(dtest.examples))
            dtest.globs.clear()
            raise aye_engine.Skip(ALL_SKIPPED)
        runner = _Runner(self.checker, flags)
        runner.examples = examples
        try:
            with self.current(test), pytest_warnings(self.settings):
                runner.run(dtest, out=_discard, clear_globs=True)
        finally:
            runner.examples = None
            test.ended_by = runner.ended_by
            test.failures = list(examples.failures)
        examples.finish()
        if examples.failures:
            raise _ExamplesFailed(examples.failures)


def _discard(text: str) -> None:
    """Where doctest's own reports would go: nowhere (``_Runner`` reports)."""


__all__: Sequence[str] = ("DoctestExecutor",)
