"""aye's own ``pytest`` module: what pytest suites import, without pytest.

When aye runs a pytest suite (on its own engine, always), the worker
installs this file's source under the name ``pytest`` (``sys.modules``) before
any conftest or test module is imported, so ``import pytest`` in user code
gets this module even when a real pytest is installed. Nothing here runs a
test: the pytest front-end (``aye_pytest_frontend``) reads what these objects
leave on the user's code (``pytestmark`` lists of ``Mark``, the
``_aye_fixture`` spec of a fixture function, ``ParameterSet``), and aye's
engine runs the model the front-end builds.

It provides, with pytest 9.1.1's behaviour and messages:

- marks: ``mark`` (a ``MarkGenerator``), ``Mark``, ``MarkDecorator``,
  ``param`` (a ``ParameterSet`` whose marks are ``Mark``\\ s), ``HIDDEN_PARAM``;
- fixtures: ``fixture`` returns the decorated function itself, with
  ``_aye_fixture`` set to a ``FixtureSpec``; ``yield_fixture``;
  ``FixtureLookupError`` (the engine's own class once ``aye_native_engine``
  is loaded, so that ``except pytest.FixtureLookupError`` catches it);
- outcomes: ``skip``, ``fail``, ``xfail``, ``exit``, ``importorskip`` and their
  exceptions (``OutcomeException``, ``Skipped``, ``Failed``, ``XFailed``,
  ``Exit``);
- checks: ``raises`` (``RaisesExc``, ``ExceptionInfo``), ``warns``,
  ``deprecated_call``, ``WarningsRecorder``, ``approx``;
- ``MonkeyPatch``, ``Stash`` and ``StashKey``;
- types for annotations and ``isinstance`` (``FixtureRequest``, ``Config``,
  ``Item``, ``Function``, ``CaptureFixture``…): abstract base classes that the
  front-end and the engine subclass, or ``register`` their own classes with;
- ``hookimpl`` and ``hookspec`` (``_aye_hookimpl`` / ``_aye_hookspec`` on the
  function), ``register_assert_rewrite`` (``_aye_rewrite_modules``),
  ``ExitCode``, ``UsageError``, the warning classes, ``__version__``;
- installed as ``pytest``, the ``_pytest.*`` modules suites import from
  (``_pytest.mark.structures``, ``_pytest.fixtures``…), as aliases of the same
  objects (``_aye_install_private_modules``), and a ``pytest`` distribution
  at ``__version__`` (``_aye_install_distribution``): with the module's spec,
  ``importlib.util.find_spec("pytest")`` and
  ``importlib.metadata.version("pytest")`` find aye's pytest, as code that
  asks whether pytest is installed does.

Hooks for the front-end: ``mark._config`` (an object with ``getini``) turns on
pytest's unknown-mark checks (``markers``, ``strict_markers``);
``_aye_rewrite_modules`` lists the names given to ``register_assert_rewrite``.

Anything else raises an ``AttributeError`` (a ``ModuleNotFoundError`` for a
``_pytest`` module) that names it and says aye's pytest module doesn't provide
it yet. ``main``, ``console_main`` and ``set_trace`` raise ``UsageError``: aye
runs the suite.

Standard library only (Python 3.10+).
"""

from __future__ import annotations

import abc
import builtins
import dataclasses
import enum
import importlib
import importlib.machinery
import inspect
import itertools
import linecache
import math
import os
import pprint
import re
import sys
import types
import warnings
from collections.abc import (
    Callable,
    Collection,
    Iterable,
    Iterator,
    Mapping,
    MutableMapping,
    Sequence,
    Sized,
)
from contextlib import contextmanager
from datetime import datetime, timedelta
from decimal import Decimal
from difflib import ndiff
from numbers import Complex
from pathlib import Path, PurePath
from traceback import format_exception, format_exception_only
from typing import (
    TYPE_CHECKING,
    Any,
    AnyStr,
    ClassVar,
    Generic,
    Literal,
    NamedTuple,
    NoReturn,
    TypeVar,
)

if TYPE_CHECKING:
    from typing import Self

__version__ = "9.1.1"
version_tuple = (9, 1, 1)
# Tells user code (and aye's own tests) that this is aye's module.
__aye__ = True
if "__file__" not in globals():
    # The worker installs the source from memory; frames read ``<aye:pytest>``.
    # As pytest's, the file is a package's ``__init__.py``: code that takes
    # ``os.path.dirname(pytest.__file__)`` as pytest's directory (rich's
    # ``Traceback(suppress=[pytest])``) gets ``<aye:pytest>``, which names
    # pytest and is where this module's frames are.
    __file__ = "<aye:pytest>/__init__.py"
if globals().get("__spec__") is None:
    # The worker installs a bare module, without a spec: then
    # ``importlib.util.find_spec("pytest")`` raises ``ValueError`` instead of
    # finding this module, and code that asks whether pytest is installed
    # (the doctest adapter, before it loads conftests) is told it isn't.
    __spec__ = importlib.machinery.ModuleSpec(__name__, None, origin=__file__)

# The module names given to ``register_assert_rewrite``, in order: the
# front-end's ``should_rewrite`` reads this list.
_aye_rewrite_modules: list[str] = []


def _groups() -> tuple[Any, Any]:
    """``BaseExceptionGroup`` and ``ExceptionGroup``: the builtins, or on
    Python 3.10 the ``exceptiongroup`` backport's when the suite loaded it (as
    pytest uses it there), else ``()`` (which nothing is an instance of)."""
    source = (
        builtins if hasattr(builtins, "BaseExceptionGroup") else sys.modules.get("exceptiongroup")
    )
    return (
        getattr(source, "BaseExceptionGroup", ()),
        getattr(source, "ExceptionGroup", ()),
    )


def _is_group(exc: object) -> bool:
    return isinstance(exc, _groups()[0])


_HOW_TO_FALL_BACK = "run the suite with pytest itself to use it"


# What an absent value is (an attribute, an item), as pytest's ``NOTSET``.
class _NotSet(enum.Enum):
    token = 0

    def __repr__(self) -> str:
        return "<NOTSET>"


NOTSET = _NotSet.token


def _not_provided(what: str) -> str:
    return f"aye's pytest module doesn't provide {what} yet ({_HOW_TO_FALL_BACK})"


def __getattr__(name: str) -> Any:
    """``FixtureLookupError`` (see ``_fixture_lookup_error``); for what
    pytest has and aye's module doesn't, an ``AttributeError`` that says so."""
    if name == "FixtureLookupError":
        return _fixture_lookup_error()
    if name.startswith("__") and name.endswith("__"):
        raise AttributeError(f"module 'pytest' has no attribute {name!r}")
    raise AttributeError(
        f"module 'pytest' has no attribute {name!r}: {_not_provided(f'pytest.{name}')}"
    )


# ------------------------------------------------------------------ warnings


class PytestWarning(UserWarning):
    """Base class for all warnings emitted by pytest."""

    __module__ = "pytest"


class PytestAssertRewriteWarning(PytestWarning):
    """Warning emitted by the pytest assert rewrite module."""

    __module__ = "pytest"


class PytestCacheWarning(PytestWarning):
    """Warning emitted by the cache plugin in various situations."""

    __module__ = "pytest"


class PytestConfigWarning(PytestWarning):
    """Warning emitted for configuration issues."""

    __module__ = "pytest"


class PytestCollectionWarning(PytestWarning):
    """Warning emitted when pytest is not able to collect a file or symbol in a module."""

    __module__ = "pytest"


class PytestDeprecationWarning(PytestWarning, DeprecationWarning):
    """Warning class for features that will be removed in a future version."""

    __module__ = "pytest"


class PytestRemovedIn9Warning(PytestDeprecationWarning):
    """Warning class for features that were removed in pytest 9 (kept for
    suites that name it)."""

    __module__ = "pytest"


class PytestRemovedIn10Warning(PytestDeprecationWarning):
    """Warning class for features that will be removed in pytest 10."""

    __module__ = "pytest"


class PytestExperimentalApiWarning(PytestWarning, FutureWarning):
    """Warning category used to denote experiments in pytest."""

    __module__ = "pytest"

    @classmethod
    def simple(cls, apiname: str) -> PytestExperimentalApiWarning:
        return cls(f"{apiname} is an experimental api that may change over time")


class PytestReturnNotNoneWarning(PytestWarning):
    """Warning emitted when a test function returns a value other than ``None``."""

    __module__ = "pytest"


class PytestUnknownMarkWarning(PytestWarning):
    """Warning emitted on use of unknown markers."""

    __module__ = "pytest"


class PytestUnraisableExceptionWarning(PytestWarning):
    """An unraisable exception was reported."""

    __module__ = "pytest"


class PytestUnhandledThreadExceptionWarning(PytestWarning):
    """An unhandled exception occurred in a ``threading.Thread``."""

    __module__ = "pytest"


class PytestFDWarning(PytestWarning):
    """When the lsof plugin finds leaked fds."""

    __module__ = "pytest"


# ------------------------------------------------------------------ session


class ExitCode(enum.IntEnum):
    """The exit codes pytest uses."""

    OK = 0
    TESTS_FAILED = 1
    INTERRUPTED = 2
    INTERNAL_ERROR = 3
    USAGE_ERROR = 4
    NO_TESTS_COLLECTED = 5
    MAX_WARNINGS_ERROR = 6

    __module__ = "pytest"


class UsageError(Exception):
    """Error in pytest usage or invocation."""

    __module__ = "pytest"


def main(args: Any = None, plugins: Any = None) -> NoReturn:
    """pytest's in-process entry point: aye runs this suite, so it can't."""
    raise UsageError(
        "pytest.main() can't run inside aye's native engine: aye runs this suite "
        f"itself ({_HOW_TO_FALL_BACK})"
    )


def console_main() -> NoReturn:
    """pytest's console entry point: aye runs this suite, so it can't."""
    raise UsageError(
        "pytest.console_main() can't run inside aye's native engine: aye runs this "
        f"suite itself ({_HOW_TO_FALL_BACK})"
    )


def set_trace(*args: Any, **kwargs: Any) -> NoReturn:
    """pytest's debugger entry point, not provided yet."""
    raise UsageError(_not_provided("pytest.set_trace()"))


cmdline = types.SimpleNamespace(main=main)


# ------------------------------------------------------------------ outcomes


class OutcomeException(BaseException):
    """OutcomeException and its subclass instances indicate and contain info
    about test and collection outcomes."""

    def __init__(self, msg: str | None = None, pytrace: bool = True) -> None:
        if msg is not None and not isinstance(msg, str):
            error_msg = (
                "{} expected string as 'msg' parameter, got '{}' instead.\n"
                "Perhaps you meant to use a mark?"
            )
            raise TypeError(error_msg.format(type(self).__name__, type(msg).__name__))
        super().__init__(msg)
        self.msg = msg
        self.pytrace = pytrace

    def __repr__(self) -> str:
        if self.msg is not None:
            return self.msg
        return f"<{self.__class__.__name__} instance>"

    __str__ = __repr__


TEST_OUTCOME = (OutcomeException, Exception)


class Skipped(OutcomeException):
    """Raised by ``pytest.skip()`` and ``pytest.importorskip()``."""

    # As pytest does, for a shorter print (and so that aye_engine.is_skip
    # knows it by its name and module).
    __module__ = "builtins"

    def __init__(
        self,
        msg: str | None = None,
        pytrace: bool = True,
        allow_module_level: bool = False,
        *,
        _use_item_location: bool = False,
    ) -> None:
        super().__init__(msg=msg, pytrace=pytrace)
        self.allow_module_level = allow_module_level
        self._use_item_location = _use_item_location


class Failed(OutcomeException):
    """Raised from an explicit call to pytest.fail()."""

    __module__ = "builtins"


class XFailed(Failed):
    """Raised from an explicit call to pytest.xfail()."""


class Exit(Exception):
    """Raised for immediate program exits (no tracebacks/summaries)."""

    def __init__(self, msg: str = "unknown reason", returncode: int | None = None) -> None:
        self.msg = msg
        self.returncode = returncode
        super().__init__(msg)


class _Exit:
    """Exit testing process."""

    Exception: ClassVar[type[Exit]] = Exit

    def __call__(self, reason: str = "", returncode: int | None = None) -> NoReturn:
        __tracebackhide__ = True
        raise Exit(msg=reason, returncode=returncode)


class _Skip:
    """Skip an executing test with the given message."""

    Exception: ClassVar[type[Skipped]] = Skipped

    def __call__(self, reason: str = "", allow_module_level: bool = False) -> NoReturn:
        __tracebackhide__ = True
        raise Skipped(msg=reason, allow_module_level=allow_module_level)


class _Fail:
    """Explicitly fail an executing test with the given message."""

    Exception: ClassVar[type[Failed]] = Failed

    def __call__(self, reason: str = "", pytrace: bool = True) -> NoReturn:
        __tracebackhide__ = True
        raise Failed(msg=reason, pytrace=pytrace)


class _XFail:
    """Imperatively xfail an executing test or setup function with the given reason."""

    Exception: ClassVar[type[XFailed]] = XFailed

    def __call__(self, reason: str = "") -> NoReturn:
        __tracebackhide__ = True
        raise XFailed(msg=reason)


exit = _Exit()
skip = _Skip()
fail = _Fail()
xfail = _XFail()


def importorskip(
    modname: str,
    minversion: str | None = None,
    reason: str | None = None,
    *,
    exc_type: type[ImportError] | None = None,
) -> Any:
    """Import and return the requested module ``modname``, or skip the
    current test if the module cannot be imported (``exc_type``, by default
    ``ModuleNotFoundError``) or its ``__version__`` is below ``minversion``
    (PEP 440)."""
    __tracebackhide__ = True
    compile(modname, "", "eval")  # to catch syntaxerrors
    if exc_type is None:
        exc_type = ModuleNotFoundError
    skipped: Skipped | None = None
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            importlib.import_module(modname)
        except exc_type as exc:
            if reason is None:
                reason = f"could not import {modname!r}: {exc}"
            skipped = Skipped(reason, allow_module_level=True)
    if skipped:
        raise skipped
    module = sys.modules[modname]
    if minversion is None:
        return module
    verattr = getattr(module, "__version__", None)
    if verattr is None or _version_key(verattr) < _version_key(minversion):
        raise Skipped(
            f"module {modname!r} has __version__ {verattr!r}, required is: {minversion!r}",
            allow_module_level=True,
        )
    return module


# PEP 440, Appendix B (what packaging.version parses).
_VERSION = re.compile(
    r"""
    v?
    (?:(?P<epoch>[0-9]+)!)?
    (?P<release>[0-9]+(?:\.[0-9]+)*)
    (?P<pre>[-_\.]?(?P<pre_l>alpha|a|beta|b|preview|pre|c|rc)[-_\.]?(?P<pre_n>[0-9]+)?)?
    (?P<post>(?:-(?P<post_n1>[0-9]+))|(?:[-_\.]?(?P<post_l>post|rev|r)[-_\.]?(?P<post_n2>[0-9]+)?))?
    (?P<dev>[-_\.]?(?P<dev_l>dev)[-_\.]?(?P<dev_n>[0-9]+)?)?
    (?:\+(?P<local>[a-z0-9]+(?:[-_\.][a-z0-9]+)*))?
    """,
    re.VERBOSE | re.IGNORECASE,
)
_PRE_LETTERS = {"alpha": "a", "beta": "b", "c": "rc", "pre": "rc", "preview": "rc"}


@dataclasses.dataclass(frozen=True)
class _Extreme:
    """Greater (``sign`` 1) or smaller (-1) than anything it is compared with."""

    sign: int

    def __lt__(self, other: object) -> bool:
        return self.sign < 0 and other is not self

    def __gt__(self, other: object) -> bool:
        return self.sign > 0 and other is not self

    def __le__(self, other: object) -> bool:
        return self.sign < 0 or other is self

    def __ge__(self, other: object) -> bool:
        return self.sign > 0 or other is self


_INFINITY = _Extreme(1)
_NEGATIVE_INFINITY = _Extreme(-1)


def _version_key(text: str) -> tuple[Any, ...]:
    """A key that orders versions as PEP 440 (and ``packaging.version``)
    does. An invalid version raises ``ValueError``."""
    found = _VERSION.fullmatch(str(text).strip())
    if found is None:
        raise ValueError(f"Invalid version: {text!r}")
    release = [int(part) for part in found["release"].split(".")]
    while len(release) > 1 and release[-1] == 0:
        release.pop()
    pre = None
    if found["pre_l"]:
        letter = found["pre_l"].lower()
        pre = (_PRE_LETTERS.get(letter, letter), int(found["pre_n"] or 0))
    post = None
    if found["post_l"] or found["post_n1"]:
        post = ("post", int(found["post_n1"] or found["post_n2"] or 0))
    dev = ("dev", int(found["dev_n"] or 0)) if found["dev_l"] else None
    if pre is None and post is None and dev is not None:
        pre_key: Any = _NEGATIVE_INFINITY
    else:
        pre_key = _INFINITY if pre is None else pre
    local: Any = _NEGATIVE_INFINITY
    if found["local"]:
        local = tuple(
            (int(part), "") if part.isdigit() else (_NEGATIVE_INFINITY, part)
            for part in re.split(r"[-_\.]", found["local"].lower())
        )
    return (
        int(found["epoch"] or 0),
        tuple(release),
        pre_key,
        _NEGATIVE_INFINITY if post is None else post,
        _INFINITY if dev is None else dev,
        local,
    )


# ------------------------------------------------------------------ marks


class _HiddenParam(enum.Enum):
    token = 0


#: Can be used as a parameter set id to hide it from the test name.
HIDDEN_PARAM = _HiddenParam.token

# The marks pytest registers itself, known without a ``markers`` line.
_BUILTIN_MARKERS = frozenset(
    (
        "filterwarnings",
        "skip",
        "skipif",
        "xfail",
        "parametrize",
        "usefixtures",
        "tryfirst",
        "trylast",
    )
)
_MARKS_ON_FIXTURES = (
    "Marks cannot be applied to fixtures.\n"
    "See docs: https://docs.pytest.org/en/stable/deprecations.html"
    "#applying-a-mark-to-a-fixture-function"
)
_PARAMETRIZE_ARGS_WITH_IDS = 4


def _istestfunc(func: object) -> bool:
    return callable(func) and getattr(func, "__name__", "<lambda>") != "<lambda>"


@dataclasses.dataclass(frozen=True)
class Mark:
    """A pytest mark: a name with the arguments the decorator was given."""

    #: Name of the mark.
    name: str
    #: Positional arguments of the mark decorator.
    args: tuple[Any, ...]
    #: Keyword arguments of the mark decorator.
    kwargs: Mapping[str, Any]
    #: Source Mark for ids with parametrize Marks.
    _param_ids_from: Mark | None = dataclasses.field(default=None, repr=False)
    #: Resolved/generated ids with parametrize Marks.
    _param_ids_generated: Sequence[str] | None = dataclasses.field(default=None, repr=False)

    def __init__(
        self,
        name: str,
        args: tuple[Any, ...],
        kwargs: Mapping[str, Any],
        param_ids_from: Mark | None = None,
        param_ids_generated: Sequence[str] | None = None,
        *,
        _ispytest: bool = False,
    ) -> None:
        object.__setattr__(self, "name", name)
        object.__setattr__(self, "args", args)
        object.__setattr__(self, "kwargs", kwargs)
        object.__setattr__(self, "_param_ids_from", param_ids_from)
        object.__setattr__(self, "_param_ids_generated", param_ids_generated)

    def _has_param_ids(self) -> bool:
        return "ids" in self.kwargs or len(self.args) >= _PARAMETRIZE_ARGS_WITH_IDS

    def combined_with(self, other: Mark) -> Mark:
        """A new Mark that appends ``other``'s args and merges its kwargs
        (``other`` wins)."""
        if self.name != other.name:
            raise ValueError(f"can't combine mark {self.name!r} with mark {other.name!r}")
        param_ids_from: Mark | None = None
        if self.name == "parametrize":
            if other._has_param_ids():
                param_ids_from = other
            elif self._has_param_ids():
                param_ids_from = self
        return Mark(
            self.name,
            self.args + other.args,
            dict(self.kwargs, **other.kwargs),
            param_ids_from=param_ids_from,
            _ispytest=True,
        )


@dataclasses.dataclass
class MarkDecorator:
    """A decorator for applying a mark on test functions and classes.

    Called with a single class or test function and nothing else, it stores
    its mark on it (``pytestmark``) and returns it; called any other way, it
    returns a new ``MarkDecorator`` with the arguments added."""

    mark: Mark

    def __init__(self, mark: Mark, *, _ispytest: bool = False) -> None:
        self.mark = mark

    @property
    def name(self) -> str:
        """Alias for mark.name."""
        return self.mark.name

    @property
    def args(self) -> tuple[Any, ...]:
        """Alias for mark.args."""
        return self.mark.args

    @property
    def kwargs(self) -> Mapping[str, Any]:
        """Alias for mark.kwargs."""
        return self.mark.kwargs

    @property
    def markname(self) -> str:
        return self.name

    def with_args(self, *args: object, **kwargs: object) -> MarkDecorator:
        """A MarkDecorator with extra arguments added, even when the sole
        argument is a function or class."""
        mark = Mark(self.name, args, kwargs, _ispytest=True)
        return MarkDecorator(self.mark.combined_with(mark), _ispytest=True)

    def __call__(self, *args: object, **kwargs: object) -> Any:
        if args and not kwargs:
            func = args[0]
            is_class = inspect.isclass(func)
            # The marks of a staticmethod or classmethod live on its function.
            unwrapped = func.__func__ if isinstance(func, staticmethod | classmethod) else func
            if len(args) == 1 and (_istestfunc(unwrapped) or is_class):
                _store_mark(unwrapped, self.mark)
                return func
        return self.with_args(*args, **kwargs)


def _normalize_mark_list(mark_list: Iterable[Mark | MarkDecorator]) -> Iterator[Mark]:
    """The ``Mark`` of each item (a ``MarkDecorator``'s ``.mark``)."""
    for item in mark_list:
        mark = getattr(item, "mark", item)
        if not isinstance(mark, Mark):
            raise TypeError(f"got {mark!r} instead of Mark")
        yield mark


def _get_unpacked_marks(obj: object, *, consider_mro: bool = True) -> list[Mark]:
    """The marks stored on ``obj``: for a class, its own and (with
    ``consider_mro``) its bases', base classes first; for anything else, its
    ``pytestmark`` (a list or a single mark)."""
    if isinstance(obj, type):
        classes = reversed(obj.__mro__) if consider_mro else (obj,)
        marks: list[Any] = []
        for cls in classes:
            item = cls.__dict__.get("pytestmark", [])
            if isinstance(item, list):
                marks.extend(item)
            else:
                marks.append(item)
    else:
        attribute = getattr(obj, "pytestmark", [])
        if attribute is None:
            warnings.warn(
                "Module defines a `__getattr__` which returns None for 'pytestmark' instead "
                "of raising AttributeError. Make sure `__getattr__` raises AttributeError "
                "for attributes it does not provide. "
                "See https://github.com/pytest-dev/pytest/issues/8265",
                PytestCollectionWarning,
                stacklevel=2,
            )
            marks = []
        elif isinstance(attribute, list):
            marks = attribute
        else:
            marks = [attribute]
    return list(_normalize_mark_list(marks))


def _store_mark(obj: Any, mark: Mark) -> None:
    """Store ``mark`` on ``obj``: a new ``pytestmark`` list with ``obj``'s own
    marks then ``mark``, never one borrowed from a base class."""
    if not isinstance(mark, Mark):
        raise TypeError(f"got {mark!r} instead of Mark")
    if _fixture_spec(obj) is not None:
        fail(_MARKS_ON_FIXTURES)
    obj.pytestmark = [*_get_unpacked_marks(obj, consider_mro=False), mark]


class MarkGenerator:
    """``pytest.mark``: any attribute is a ``MarkDecorator`` of that name.

    With ``_config`` set (an object with ``getini``), a name that isn't a
    registered marker warns ``PytestUnknownMarkWarning``, or fails with
    ``strict_markers``, as pytest does."""

    def __init__(self, *, _ispytest: bool = False) -> None:
        self._config: Any = None
        self._markers: set[str] = set(_BUILTIN_MARKERS)
        # The config ``_markers`` was read from: another one (the next
        # batch's, in a warm worker) starts again from the builtins.
        self._markers_of: Any = None

    def __getattr__(self, name: str) -> MarkDecorator:
        if name[0] == "_":
            raise AttributeError("Marker name must NOT start with underscore")
        if self._config is not None:
            self._check_registered(name)
        return MarkDecorator(Mark(name, (), {}, _ispytest=True), _ispytest=True)

    def _check_registered(self, name: str) -> None:
        if self._markers_of is not self._config:
            self._markers = set(_BUILTIN_MARKERS)
            self._markers_of = self._config
        if name not in self._markers:
            for line in self._config.getini("markers") or ():
                self._markers.add(line.split(":")[0].split("(")[0].strip())
        if name in self._markers:
            return
        if name in ("parameterize", "parametrise", "parameterise"):
            __tracebackhide__ = True
            fail(f"Unknown '{name}' mark, did you mean 'parametrize'?")
        strict = self._config.getini("strict_markers")
        if strict is None:
            strict = self._config.getini("strict")
        if strict:
            fail(f"{name!r} not found in `markers` configuration option", pytrace=False)
        warnings.warn(
            f"Unknown pytest.mark.{name} - is this a typo?  You can register "
            "custom marks to avoid this warning - for details, see "
            "https://docs.pytest.org/en/stable/how-to/mark.html",
            PytestUnknownMarkWarning,
            3,
        )


mark = MarkGenerator(_ispytest=True)
MARK_GEN = mark


class ParameterSet(NamedTuple):
    """A set of values for a set of parameters, with marks (``Mark``\\ s) and
    an optional id: what ``pytest.param`` returns."""

    values: Sequence[object]
    marks: Collection[Mark]
    id: str | _HiddenParam | None

    @classmethod
    def param(
        cls,
        *values: object,
        marks: MarkDecorator | Mark | Collection[MarkDecorator | Mark] = (),
        id: str | _HiddenParam | None = None,
    ) -> ParameterSet:
        if isinstance(marks, MarkDecorator | Mark):
            marks = (marks,)
        elif not isinstance(marks, Collection) or isinstance(marks, str | bytes):
            raise TypeError(
                f"pytest.param(marks=...) takes a mark or a collection of marks, got {marks!r}"
            )
        normalized = tuple(_normalize_mark_list(marks))
        if any(m.name == "usefixtures" for m in normalized):
            raise ValueError(
                "pytest.param cannot add pytest.mark.usefixtures; see "
                "https://docs.pytest.org/en/stable/reference/reference.html#pytest-param"
            )
        if id is not None and not isinstance(id, str) and id is not HIDDEN_PARAM:
            raise TypeError(
                "Expected id to be a string or a `pytest.HIDDEN_PARAM` sentinel, "
                f"got {type(id)}: {id!r}",
            )
        return cls(values, normalized, id)

    @classmethod
    def extract_from(cls, parameterset: object, force_tuple: bool = False) -> ParameterSet:
        """``parameterset`` as a ``ParameterSet``: itself if it is one; with
        ``force_tuple`` (a single argname without a trailing comma) the
        value as the only one; else the value as the tuple of values."""
        if isinstance(parameterset, cls):
            return parameterset
        if force_tuple:
            return cls.param(parameterset)
        return cls(parameterset, (), None)  # type: ignore[arg-type]

    @staticmethod
    def _parse_parametrize_args(
        argnames: str | Sequence[str], argvalues: object, *args: object, **kwargs: object
    ) -> tuple[Sequence[str], bool]:
        if isinstance(argnames, str):
            has_trailing_comma = argnames.rstrip().endswith(",")
            argnames = [x.strip() for x in argnames.split(",") if x.strip()]
            force_tuple = len(argnames) == 1 and not has_trailing_comma
        else:
            force_tuple = False
        return argnames, force_tuple


def param(
    *values: object,
    marks: MarkDecorator | Mark | Collection[MarkDecorator | Mark] = (),
    id: str | _HiddenParam | None = None,
) -> ParameterSet:
    """Specify a parameter in ``pytest.mark.parametrize`` calls or
    parametrized fixtures, with its own marks and id."""
    return ParameterSet.param(*values, marks=marks, id=id)


# ------------------------------------------------------------------ fixtures


ScopeName = Literal["session", "package", "module", "class", "function"]


@dataclasses.dataclass(frozen=True)
class FixtureSpec:
    """What ``@pytest.fixture(...)`` declares, stored on the fixture function
    as ``_aye_fixture``. ``scope`` is a scope name, or pytest's dynamic scope
    (a callable taking the fixture name and the config). Calling it with a
    function marks the function (``@pytest.fixture(scope=...)``)."""

    scope: str | Callable[[str, Any], str] = "function"
    params: list[Any] | None = None
    autouse: bool = False
    ids: tuple[Any, ...] | Callable[[Any], object | None] | None = None
    name: str | None = None

    def __call__(self, function: Any) -> Any:
        __tracebackhide__ = True
        if inspect.isclass(function):
            raise ValueError("class fixtures not supported (maybe in the future)")
        if not callable(function) and not isinstance(function, staticmethod | classmethod):
            raise TypeError(f"@pytest.fixture takes a function, got {function!r}")
        if _fixture_spec(function) is not None:
            raise ValueError(
                "@pytest.fixture is being applied more than once to the same function "
                f"{function.__name__!r}"
            )
        if hasattr(function, "pytestmark"):
            fail(_MARKS_ON_FIXTURES)
        name = self.name or function.__name__
        if name == "request":
            fail(
                f"'request' is a reserved word for fixtures, use another name:\n"
                f"  {_location(function)}",
                pytrace=False,
            )
        function._aye_fixture = self
        return function


def _fixture_spec(obj: object) -> FixtureSpec | None:
    """The ``FixtureSpec`` of a fixture function, else ``None``."""
    try:
        spec = getattr(obj, "_aye_fixture", None)
    except Exception:  # noqa: BLE001 - an object whose getattr breaks is no fixture
        return None
    return spec if isinstance(spec, FixtureSpec) else None


def _location(function: Any) -> str:
    """``path:line`` as pytest's ``getlocation`` writes it (one past the
    ``def`` line)."""
    try:
        real = inspect.unwrap(function)
        return f"{Path(inspect.getfile(real))}:{real.__code__.co_firstlineno + 1}"
    except Exception:  # noqa: BLE001 - a location is only for the message
        return repr(function)


def fixture(
    fixture_function: Any = None,
    *,
    scope: str | Callable[[str, Any], str] = "function",
    params: Iterable[object] | None = None,
    autouse: bool = False,
    ids: Sequence[object | None] | Callable[[Any], object | None] | None = None,
    name: str | None = None,
) -> Any:
    """Decorator to mark a fixture factory function, bare or with
    arguments. It returns the function itself, with ``_aye_fixture`` set to
    its ``FixtureSpec``; called with arguments only, it returns the spec,
    which marks the function it is called with."""
    spec = FixtureSpec(
        scope=scope,
        params=list(params) if params is not None else None,
        autouse=autouse,
        ids=None if ids is None else ids if callable(ids) else tuple(ids),
        name=name,
    )
    if fixture_function:
        return spec(fixture_function)
    return spec


def yield_fixture(
    fixture_function: Any = None,
    *args: Any,
    scope: str | Callable[[str, Any], str] = "function",
    params: Iterable[object] | None = None,
    autouse: bool = False,
    ids: Sequence[object | None] | Callable[[Any], object | None] | None = None,
    name: str | None = None,
) -> Any:
    """Deprecated alias of ``fixture``."""
    warnings.warn(
        PytestDeprecationWarning(
            "@pytest.yield_fixture is deprecated.\nUse @pytest.fixture instead; they are the same."
        ),
        stacklevel=2,
    )
    return fixture(
        fixture_function, *args, scope=scope, params=params, autouse=autouse, ids=ids, name=name
    )


class _FixtureLookupError(LookupError):
    """Could not return a requested fixture (missing or invalid)."""

    def __init__(self, argname: str | None, request: Any = None, msg: str | None = None) -> None:
        super().__init__(msg or f"fixture {argname!r} not found")
        self.argname = argname
        self.request = request
        self.msg = msg

    def __str__(self) -> str:
        return self.msg or f"fixture {self.argname!r} not found"


_FixtureLookupError.__name__ = _FixtureLookupError.__qualname__ = "FixtureLookupError"


def _fixture_lookup_error() -> type[LookupError]:
    """``pytest.FixtureLookupError``: the class aye's engine raises when it is
    loaded (``aye_native_engine``, installed before this module), so that
    ``except pytest.FixtureLookupError`` catches its errors (an ``except``
    matches real subclasses only); else this module's own."""
    engine_error = getattr(sys.modules.get("aye_native_engine"), "FixtureLookupError", None)
    if isinstance(engine_error, type) and issubclass(engine_error, LookupError):
        return engine_error
    return _FixtureLookupError


# ------------------------------------------------------------------ types


class Node(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """pytest's base node type. The front-end's items and collectors
    subclass these types (or ``register`` with them), so that annotations
    and ``isinstance`` work."""


class Collector(Node):
    """Base collector type."""

    class CollectError(Exception):
        """An error during collection, contains a custom message."""


class FSCollector(Collector):
    """Base type of collectors for files and directories."""


class File(FSCollector):
    """Base type of collectors for test files."""


class PyCollector(Collector):
    """Base type of collectors for Python objects."""


class Module(File, PyCollector):
    """Collector for a test module."""


class Class(PyCollector):
    """Collector for a test class."""


class Directory(FSCollector):
    """Base type of directory collectors."""


class Dir(Directory):
    """Collector for a directory without ``__init__.py``."""


class Package(Directory):
    """Collector for a directory with ``__init__.py``."""


class Session(Collector):
    """The root of the collection tree."""

    class Interrupted(KeyboardInterrupt):
        """Signals that the test run was interrupted."""

        __module__ = "builtins"

    class Failed(Exception):
        """Signals a stop as failed test run."""


class Item(Node):
    """Base type of test items."""

    nextitem = None


class Function(Item):
    """A test function or method."""


class FunctionDefinition(Function):
    """The item ``Metafunc.definition`` is."""


class DoctestItem(Item):
    """A doctest (aye's doctest adapter runs them; no front-end item is one)."""


class Config(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """The front-end's config object subclasses this type."""


class PytestPluginManager(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Type of ``config.pluginmanager``."""


class Parser(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Type of the ``parser`` that ``pytest_addoption`` receives."""


class OptionGroup(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Type of ``parser.getgroup(...)``."""


class Metafunc(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Type of the ``metafunc`` that ``pytest_generate_tests`` receives."""


class FixtureRequest(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Type of the ``request`` fixture (the engine registers its own)."""


class TopRequest(FixtureRequest):
    """The ``request`` a test function receives."""


class SubRequest(FixtureRequest):
    """The ``request`` a fixture function receives."""


class CaptureFixture(abc.ABC, Generic[AnyStr]):
    """Type of ``capsys``, ``capfd`` and their binary forms."""


class LogCaptureFixture(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Type of ``caplog``."""


class TempPathFactory(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Type of ``tmp_path_factory``."""


class Cache(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Type of ``config.cache`` and the ``cache`` fixture."""


_T = TypeVar("_T")
_D = TypeVar("_D")


class CallInfo(abc.ABC, Generic[_T]):
    """Result or exception of a phase, in report hooks."""


class TestReport(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Report of a test phase, in report hooks."""

    __test__ = False


class CollectReport(abc.ABC):  # noqa: B024 - a type for isinstance and annotations
    """Report of a collector, in report hooks."""


class StashKey(Generic[_T]):
    """A key of a ``Stash``: unique, typed by the value it holds."""

    __slots__ = ()


class Stash:
    """A type-safe heterogeneous mutable mapping keyed by ``StashKey``\\ s."""

    __slots__ = ("_storage",)

    def __init__(self) -> None:
        self._storage: dict[StashKey[Any], object] = {}

    def __setitem__(self, key: StashKey[_T], value: _T) -> None:
        self._storage[key] = value

    def __getitem__(self, key: StashKey[_T]) -> _T:
        return self._storage[key]  # type: ignore[return-value]

    def get(self, key: StashKey[_T], default: _D) -> _T | _D:
        try:
            return self[key]
        except KeyError:
            return default

    def setdefault(self, key: StashKey[_T], default: _T) -> _T:
        try:
            return self[key]
        except KeyError:
            self[key] = default
            return default

    def __delitem__(self, key: StashKey[_T]) -> None:
        del self._storage[key]

    def __contains__(self, key: StashKey[_T]) -> bool:
        return key in self._storage

    def __len__(self) -> int:
        return len(self._storage)


# ------------------------------------------------------------------ hooks


_HOOKIMPL_OPTIONS = frozenset(
    ("wrapper", "hookwrapper", "optionalhook", "tryfirst", "trylast", "specname")
)
_HOOKSPEC_OPTIONS = frozenset(("firstresult", "historic", "warn_on_impl", "warn_on_impl_args"))


def _hook_marker(attribute: str, allowed: frozenset[str], what: str) -> Callable[..., Any]:
    def marker(function: Any = None, **opts: Any) -> Any:
        unknown = sorted(set(opts) - allowed)
        if unknown:
            raise TypeError(f"{what}() got unexpected keyword arguments: {', '.join(unknown)}")

        def store(func: Any) -> Any:
            setattr(func, attribute, dict(opts))
            return func

        return store if function is None else store(function)

    marker.__name__ = marker.__qualname__ = what
    marker.__doc__ = (
        f"Decorator (bare or called with options) that sets ``{attribute}`` to the "
        "options on the function and returns it."
    )
    return marker


hookimpl = _hook_marker("_aye_hookimpl", _HOOKIMPL_OPTIONS, "hookimpl")
hookspec = _hook_marker("_aye_hookspec", _HOOKSPEC_OPTIONS, "hookspec")


def register_assert_rewrite(*names: str) -> None:
    """Register one or more module names to be rewritten on import (aye's
    rewriting reads ``_aye_rewrite_modules``)."""
    for name in names:
        if not isinstance(name, str):
            msg = "expected module names as *args, got {0} instead"  # type: ignore[unreachable]
            raise TypeError(msg.format(repr(names)))
    _aye_rewrite_modules.extend(names)


# ------------------------------------------------------------------ raises

_E_co = TypeVar("_E_co", bound=BaseException, covariant=True)

# String patterns default to including the unicode flag.
_REGEX_NO_FLAGS = re.compile(r"").flags
_SAFEREPR_SIZE = 240
_DIFF_CONTEXT = 42


def _saferepr(obj: object, maxsize: int = _SAFEREPR_SIZE) -> str:
    """``repr(obj)``, never raising, cut in the middle to ``maxsize``."""
    try:
        text = repr(obj)
    except Exception as exc:  # noqa: BLE001 - a repr must not fail
        text = f"<[{type(exc).__name__} raised in repr()] {type(obj).__name__} object at 0x{id(obj):x}>"
    if len(text) > maxsize:
        start = max(0, (maxsize - 3) // 2)
        end = max(0, maxsize - 3 - start)
        text = text[:start] + "..." + text[len(text) - end :]
    return text


def _stringify_exception(exc: BaseException, include_subexception_msg: bool = True) -> str:
    """The exception's message with its ``__notes__``, as ``match`` sees it."""
    notes = getattr(exc, "__notes__", [])
    if not include_subexception_msg and _is_group(exc):
        message = exc.message
    else:
        message = str(exc)
    return "\n".join([message, *notes])


class Code:
    """A code object, as pytest's traceback entries show it."""

    __slots__ = ("raw",)

    def __init__(self, obj: types.CodeType) -> None:
        self.raw = obj

    @property
    def firstlineno(self) -> int:
        return self.raw.co_firstlineno - 1

    @property
    def name(self) -> str:
        return self.raw.co_name

    @property
    def path(self) -> Path | str:
        """The source file, or the file name when no such file exists."""
        filename = self.raw.co_filename
        if not filename:
            return ""
        try:
            path = Path(os.path.abspath(filename))
            if path.exists():
                return path
        except (OSError, ValueError):
            pass
        return filename


class Frame:
    """A frame, as pytest's traceback entries show it."""

    __slots__ = ("raw",)

    def __init__(self, frame: types.FrameType) -> None:
        self.raw = frame

    @property
    def lineno(self) -> int:
        return self.raw.f_lineno - 1

    @property
    def f_globals(self) -> dict[str, Any]:
        return self.raw.f_globals

    @property
    def f_locals(self) -> Mapping[str, Any]:
        return self.raw.f_locals

    @property
    def code(self) -> Code:
        return Code(self.raw.f_code)


def _item(namespace: Any, key: str) -> Any:
    """``namespace[key]``, or ``NOTSET`` when it raises (a frame's
    namespaces may be anything)."""
    try:
        return namespace[key]
    except Exception:  # noqa: BLE001 - see above
        return NOTSET


class TracebackEntry:
    """One entry of an ``ExceptionInfo``'s traceback (``lineno`` is 0-based,
    as pytest's)."""

    __slots__ = ("_rawentry",)

    def __init__(self, rawentry: types.TracebackType) -> None:
        self._rawentry = rawentry

    @property
    def lineno(self) -> int:
        return self._rawentry.tb_lineno - 1

    @property
    def frame(self) -> Frame:
        return Frame(self._rawentry.tb_frame)

    @property
    def relline(self) -> int:
        return self.lineno - self.frame.code.firstlineno

    @property
    def path(self) -> Path | str:
        return self.frame.code.path

    @property
    def locals(self) -> Mapping[str, Any]:
        return self.frame.f_locals

    @property
    def name(self) -> str:
        return self.frame.code.raw.co_name

    def getfirstlinesource(self) -> int:
        return self.frame.code.firstlineno

    def ishidden(self, excinfo: ExceptionInfo[BaseException] | None = None) -> bool:
        """Whether the frame sets ``__tracebackhide__`` (or a callable of it
        says so)."""
        hide: Any = _item(self.frame.f_locals, "__tracebackhide__")
        if hide is NOTSET:
            hide = _item(self.frame.f_globals, "__tracebackhide__")
        if hide is NOTSET:
            return False
        if hide and callable(hide):
            return bool(hide(excinfo))
        return bool(hide)

    def __repr__(self) -> str:
        return f"<TracebackEntry {self.frame.code.path}:{self.lineno + 1}>"

    def __str__(self) -> str:
        filename = self.frame.code.raw.co_filename
        line = linecache.getline(filename, self.lineno + 1).strip() or "???"
        return f"  File '{self.path}':{self.lineno + 1} in {self.name}\n  {line}\n"


class Traceback(list):  # type: ignore[type-arg]
    """The entries of a traceback, outermost first."""

    def __init__(self, tb: types.TracebackType | Iterable[TracebackEntry]) -> None:
        if isinstance(tb, types.TracebackType):

            def entries(current: types.TracebackType | None) -> Iterator[TracebackEntry]:
                while current is not None:
                    yield TracebackEntry(current)
                    current = current.tb_next

            super().__init__(entries(tb))
        else:
            super().__init__(tb)

    def __getitem__(self, key: Any) -> Any:
        if isinstance(key, slice):
            return self.__class__(super().__getitem__(key))
        return super().__getitem__(key)

    def cut(
        self,
        path: Any = None,
        lineno: int | None = None,
        firstlineno: int | None = None,
        excludepath: Any = None,
    ) -> Traceback:
        """The traceback from the first entry matching ``path``,
        ``lineno`` and ``firstlineno`` (and not under ``excludepath``)."""
        path_ = None if path is None else os.fspath(path)
        excludepath_ = None if excludepath is None else os.fspath(excludepath)
        for entry in self:
            code = entry.frame.code
            codepath = code.path
            if path is not None and str(codepath) != path_:
                continue
            if (
                excludepath is not None
                and isinstance(codepath, Path)
                and excludepath_ in (str(p) for p in codepath.parents)
            ):
                continue
            if lineno is not None and entry.lineno != lineno:
                continue
            if firstlineno is not None and code.firstlineno != firstlineno:
                continue
            return Traceback(entry._rawentry)
        return self

    def filter(self, excinfo_or_fn: Any, /) -> Traceback:
        """The entries an ``ExceptionInfo`` doesn't hide, or those a
        function accepts."""
        if isinstance(excinfo_or_fn, ExceptionInfo):
            excinfo = excinfo_or_fn
            return Traceback(entry for entry in self if not entry.ishidden(excinfo))
        return Traceback(filter(excinfo_or_fn, self))

    def recursionindex(self) -> int | None:
        """The index of the entry where recursion starts, if any."""
        cache: dict[tuple[Any, int, int], list[dict[str, Any]]] = {}
        for index, entry in enumerate(self):
            key = entry.frame.code.path, id(entry.frame.code.raw), entry.lineno
            seen = cache.setdefault(key, [])
            frame_locals = dict(entry.frame.f_locals)
            if any(other == frame_locals for other in seen):
                return index
            seen.append(frame_locals)
        return None


class _TextRepr:
    """What ``ExceptionInfo.getrepr`` returns: the traceback as Python prints it."""

    def __init__(self, text: str) -> None:
        self.text = text

    def __str__(self) -> str:
        return self.text

    def __repr__(self) -> str:
        return f"<{self.__class__.__name__} {self.text!r}>"


class ExceptionInfo(Generic[_E_co]):
    """Wraps ``sys.exc_info()`` objects: what ``pytest.raises`` gives."""

    _assert_start_repr: ClassVar[str] = "AssertionError('assert "

    def __init__(
        self,
        excinfo: tuple[type[_E_co], _E_co, types.TracebackType] | None,
        striptext: str = "",
        traceback: Traceback | None = None,
        *,
        _ispytest: bool = False,
    ) -> None:
        self._excinfo = excinfo
        self._striptext = striptext
        self._traceback = traceback

    @classmethod
    def from_exception(cls, exception: _E_co, exprinfo: str | None = None) -> ExceptionInfo[_E_co]:  # type: ignore[misc]
        """An ExceptionInfo for an exception that was raised (it has a
        ``__traceback__``)."""
        if not exception.__traceback__:
            raise AssertionError(
                "Exceptions passed to ExcInfo.from_exception(...) must have a non-None "
                "__traceback__."
            )
        return cls.from_exc_info((type(exception), exception, exception.__traceback__), exprinfo)

    @classmethod
    def from_exc_info(
        cls, exc_info: tuple[type[_E_co], _E_co, types.TracebackType], exprinfo: str | None = None
    ) -> ExceptionInfo[_E_co]:
        """Like ``from_exception``, from an exc_info tuple."""
        striptext = ""
        if exprinfo is None and isinstance(exc_info[1], AssertionError):
            exprinfo = getattr(exc_info[1], "msg", None)
            if exprinfo is None:
                exprinfo = _saferepr(exc_info[1])
            if exprinfo and exprinfo.startswith(cls._assert_start_repr):
                striptext = "AssertionError: "
        return cls(exc_info, striptext, _ispytest=True)

    @classmethod
    def from_current(cls, exprinfo: str | None = None) -> ExceptionInfo[BaseException]:
        """An ExceptionInfo for the exception being handled."""
        current = sys.exc_info()
        if current[0] is None or current[1] is None or current[2] is None:
            raise AssertionError("no current exception")
        return ExceptionInfo.from_exc_info((current[0], current[1], current[2]), exprinfo)

    @classmethod
    def for_later(cls) -> ExceptionInfo[_E_co]:
        """An unfilled ExceptionInfo."""
        return cls(None, _ispytest=True)

    def fill_unfilled(self, exc_info: tuple[type[_E_co], _E_co, types.TracebackType]) -> None:
        """Fill an unfilled ExceptionInfo created with ``for_later()``."""
        if self._excinfo is not None:
            raise AssertionError("ExceptionInfo was already filled")
        self._excinfo = exc_info

    def _filled(self, what: str) -> tuple[type[_E_co], _E_co, types.TracebackType]:
        if self._excinfo is None:
            raise AssertionError(f".{what} can only be used after the context manager exits")
        return self._excinfo

    @property
    def type(self) -> type[_E_co]:
        """The exception class."""
        return self._filled("type")[0]

    @property
    def value(self) -> _E_co:
        """The exception value."""
        return self._filled("value")[1]

    @property
    def tb(self) -> types.TracebackType:
        """The exception raw traceback."""
        return self._filled("tb")[2]

    @property
    def typename(self) -> str:
        """The type name of the exception."""
        return self._filled("typename")[0].__name__

    @property
    def traceback(self) -> Traceback:
        """The traceback's entries."""
        if self._traceback is None:
            self._traceback = Traceback(self.tb)
        return self._traceback

    @traceback.setter
    def traceback(self, value: Traceback) -> None:
        self._traceback = value

    def __repr__(self) -> str:
        if self._excinfo is None:
            return "<ExceptionInfo for raises contextmanager>"
        return (
            f"<{self.__class__.__name__} {_saferepr(self._excinfo[1])} tblen={len(self.traceback)}>"
        )

    def exconly(self, tryshort: bool = False) -> str:
        """The exception as a string: ``"<type>: <message>"`` (and its notes);
        ``tryshort`` strips ``AssertionError: `` from an assertion's."""
        value: Any = self.value
        if tryshort and _is_group(value):
            single = _single_subexception(value)
            if single is not None:
                return f"{single!r} [single exception in {type(value).__name__}]"
        text = "".join(format_exception_only(value)).rstrip()
        if tryshort and text.startswith(self._striptext):
            text = text[len(self._striptext) :]
        return text

    def errisinstance(self, exc: type[BaseException] | tuple[type[BaseException], ...]) -> bool:
        """Whether the exception is an instance of ``exc``."""
        return isinstance(self.value, exc)

    def getrepr(self, *args: Any, **kwargs: Any) -> _TextRepr:
        """The exception and its traceback, as Python prints them (pytest's
        styles and options aren't reproduced)."""
        return _TextRepr("".join(format_exception(self.type, self.value, self.tb)))

    def match(self, regexp: str | re.Pattern[str]) -> Literal[True]:
        """Check that ``regexp`` matches the exception's message (with its
        notes) with ``re.search``; raise ``AssertionError`` otherwise."""
        __tracebackhide__ = True
        value = _stringify_exception(self.value)
        msg = (
            f"Regex pattern did not match.\n"
            f"  Expected regex: {regexp!r}\n"
            f"  Actual message: {value!r}"
        )
        if regexp == value:
            msg += "\n Did you mean to `re.escape()` the regex?"
        if not re.search(regexp, value):
            raise AssertionError(msg)
        return True

    def _group_contains(
        self,
        exc_group: Any,
        expected_exception: Any,
        match: str | re.Pattern[str] | None,
        target_depth: int | None = None,
        current_depth: int = 1,
    ) -> bool:
        if target_depth is not None and current_depth > target_depth:
            return False
        for exc in exc_group.exceptions:
            if _is_group(exc) and self._group_contains(
                exc, expected_exception, match, target_depth, current_depth + 1
            ):
                return True
            if target_depth is not None and current_depth != target_depth:
                continue
            if not isinstance(exc, expected_exception):
                continue
            if match is not None and not re.search(match, _stringify_exception(exc)):
                continue
            return True
        return False

    def group_contains(
        self,
        expected_exception: Any,
        *,
        match: str | re.Pattern[str] | None = None,
        depth: int | None = None,
    ) -> bool:
        """Whether the captured exception group contains a matching
        exception (at ``depth``, when given)."""
        is_group = _is_group(self.value)
        if not is_group:
            raise AssertionError("Captured exception is not an instance of `BaseExceptionGroup`")
        if depth is not None and depth < 1:
            raise AssertionError("`depth` must be >= 1 if specified")
        return self._group_contains(self.value, expected_exception, match, depth)


def _single_subexception(group: Any) -> BaseException | None:
    if len(group.exceptions) != 1:
        return None
    exc = group.exceptions[0]
    if _is_group(exc):
        return _single_subexception(exc)
    return exc


def _match_pattern(match: re.Pattern[str]) -> str | re.Pattern[str]:
    """The pattern's text, unless it has flags of its own."""
    return match.pattern if match.flags == _REGEX_NO_FLAGS else match


def _exception_type_name(e: type[BaseException] | tuple[type[BaseException], ...]) -> str:
    if isinstance(e, type):
        return e.__name__
    if len(e) == 1:
        return e[0].__name__
    return "(" + ", ".join(ee.__name__ for ee in e) + ")"


def _check_raw_type(
    expected_type: type[BaseException] | tuple[type[BaseException], ...] | None,
    exception: BaseException,
) -> str | None:
    if expected_type is None or expected_type == ():
        return None
    if not isinstance(exception, expected_type):
        actual = "`" + _exception_type_name(type(exception)) + "()`"
        expected = "`" + _exception_type_name(expected_type) + "`"
        if (
            _is_group(exception)
            and isinstance(expected_type, type)
            and not issubclass(expected_type, _groups()[0])
        ):
            return f"Unexpected nested {actual}, expected {expected}"
        return f"{actual} is not an instance of {expected}"
    return None


def _is_fully_escaped(s: str) -> bool:
    # Not compiled with re.VERBOSE, so whitespace needn't be escaped.
    stripped = re.sub(r"\\.", "", s)
    return not any(c in "{}()+.*?^$[]|" for c in stripped)


def _unescape(s: str) -> str:
    return re.sub(r"\\([{}()+-.*?^$\[\]\s\\|])", r"\1", s)


def _diff_text(left: str, right: str) -> Iterator[str]:
    """pytest's explanation of two different texts (without ``-v``):
    identical ends skipped, then an ``ndiff`` of ``right`` against ``left``."""
    i = 0
    for i in range(min(len(left), len(right))):
        if left[i] != right[i]:
            break
    if i > _DIFF_CONTEXT:
        i -= 10
        yield f"Skipping {i} identical leading characters in diff, use -v to show"
        left = left[i:]
        right = right[i:]
    if len(left) == len(right):
        for i in range(len(left)):
            if left[-i] != right[-i]:
                break
        if i > _DIFF_CONTEXT:
            i -= 10
            yield f"Skipping {i} identical trailing characters in diff, use -v to show"
            left = left[:-i]
            right = right[:-i]
    if left.isspace() or right.isspace():
        left = repr(str(left))
        right = repr(str(right))
        yield "Strings contain only whitespace, escaping them using repr()"
    yield from "\n".join(
        line.strip("\n") for line in ndiff(right.splitlines(True), left.splitlines(True))
    ).splitlines()


class RaisesExc(Generic[_E_co]):
    """What ``pytest.raises`` returns: the expected exception type(s), a
    ``match`` regex and a ``check`` callable, as a context manager."""

    def __init__(
        self,
        expected_exception: Any = None,
        /,
        *,
        match: str | re.Pattern[str] | None = None,
        check: Callable[[Any], bool] | None = None,
    ) -> None:
        __tracebackhide__ = True
        if isinstance(match, str):
            error = None
            try:
                self.match: re.Pattern[str] | None = re.compile(match)
            except re.error as exc:
                error = exc
            if error is not None:
                fail(f"Invalid regex pattern provided to 'match': {error}")
            if match == "":
                warnings.warn(
                    PytestWarning(
                        "matching against an empty string will *always* pass. If you want "
                        "to check for an empty message you need to pass '^$'. If you don't "
                        "want to match you should pass `None` or leave out the parameter."
                    ),
                    stacklevel=2,
                )
        else:
            self.match = match
        # A fully escaped ``^…$`` pattern fails with a diff.
        self.rawmatch: str | None = None
        raw = match
        if isinstance(raw, re.Pattern) and raw.flags == _REGEX_NO_FLAGS:
            raw = raw.pattern
        if (
            isinstance(raw, str)
            and raw
            and raw[0] == "^"
            and raw[-1] == "$"
            and _is_fully_escaped(raw[1:-1])
        ):
            self.rawmatch = _unescape(raw[1:-1])
        self.check = check
        self._fail_reason: str | None = None
        self._nested = False
        self.is_baseexception = False
        if isinstance(expected_exception, tuple):
            expected = expected_exception
        elif expected_exception is None:
            expected = ()
        else:
            expected = (expected_exception,)
        if expected == () and match is None and check is None:
            raise ValueError("You must specify at least one parameter to match on.")
        self.expected_exceptions = tuple(self._parse_exc(e) for e in expected)
        self._just_propagate = False
        self.excinfo: ExceptionInfo[Any] | None = None

    def _parse_exc(self, exc: Any) -> type[BaseException]:
        if isinstance(exc, type) and issubclass(exc, BaseException):
            if not issubclass(exc, Exception):
                self.is_baseexception = True
            return exc
        origin = getattr(exc, "__origin__", None)
        base_group, group = _groups()
        if isinstance(origin, type) and issubclass(origin, base_group):
            (exc_type,) = exc.__args__
            if (issubclass(origin, group) and exc_type in (Exception, Any)) or (
                issubclass(origin, base_group) and exc_type in (BaseException, Any)
            ):
                if not issubclass(origin, group):
                    self.is_baseexception = True
                return origin
            raise ValueError(
                "Only `ExceptionGroup[Exception]` or `BaseExceptionGroup[BaseException]` "
                f"are accepted as generic types but got `{exc}`. "
                "As `raises` will catch all instances of the specified group regardless of the "
                "generic argument specific nested exceptions has to be checked "
                "with `RaisesGroup`."
            )
        msg = "Expected a BaseException type, but got "
        is_type = isinstance(exc, type)
        if is_type:
            raise ValueError(msg + f"{exc.__name__!r}")
        if isinstance(exc, BaseException):
            raise TypeError(msg + f"an exception instance: {type(exc).__name__}")
        raise TypeError(msg + repr(type(exc).__name__))

    @property
    def fail_reason(self) -> str | None:
        """Why the last ``matches`` failed."""
        return self._fail_reason

    def _check_check(self, exception: BaseException) -> bool:
        if self.check is None or self.check(exception):
            return True
        check_repr = "" if self._nested else " " + repr(self.check)
        self._fail_reason = f"check{check_repr} did not return True"
        return False

    def _check_match(self, e: BaseException) -> bool:
        stringified = _stringify_exception(e, include_subexception_msg=False)
        if self.match is None or re.search(self.match, stringified):
            return True
        maybe_specify_type = f" the `{_exception_type_name(type(e))}()`" if _is_group(e) else ""
        if isinstance(self.rawmatch, str):
            diff = list(_diff_text(self.rawmatch, stringified))
            self._fail_reason = ("\n" if diff and diff[0][0] == "-" else "") + "\n".join(diff)
            return False
        self._fail_reason = (
            f"Regex pattern did not match{maybe_specify_type}.\n"
            f"  Expected regex: {_match_pattern(self.match)!r}\n"
            f"  Actual message: {stringified!r}"
        )
        if _match_pattern(self.match) == stringified:
            self._fail_reason += "\n Did you mean to `re.escape()` the regex?"
        return False

    def matches(self, exception: BaseException | None) -> bool:
        """Whether ``exception`` matches; if not, ``fail_reason`` says why."""
        self._just_propagate = False
        if exception is None:
            self._fail_reason = "exception is None"
            return False
        self._fail_reason = _check_raw_type(self.expected_exceptions, exception)
        if self._fail_reason is not None:
            self._just_propagate = True
            return False
        if not self._check_match(exception):
            return False
        return self._check_check(exception)

    def __repr__(self) -> str:
        parameters = []
        if self.expected_exceptions:
            parameters.append(_exception_type_name(self.expected_exceptions))
        if self.match is not None:
            parameters.append(f"match={_match_pattern(self.match)!r}")
        if self.check is not None:
            parameters.append(f"check={self.check!r}")
        return f"RaisesExc({', '.join(parameters)})"

    def __enter__(self) -> ExceptionInfo[Any]:
        self.excinfo = ExceptionInfo.for_later()
        return self.excinfo

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: types.TracebackType | None,
    ) -> bool:
        __tracebackhide__ = True
        if exc_type is None:
            if not self.expected_exceptions:
                fail("DID NOT RAISE any exception")
            if len(self.expected_exceptions) == 1:
                fail(f"DID NOT RAISE {self.expected_exceptions[0].__name__}")
            names = ", ".join(x.__name__ for x in self.expected_exceptions)
            fail(f"DID NOT RAISE any of ({names})")
        if self.excinfo is None:
            raise AssertionError("Internal error - should have been constructed in __enter__")
        if not self.matches(exc_val):
            if self._just_propagate:
                return False
            raise AssertionError(self._fail_reason) from None
        self.excinfo.fill_unfilled((exc_type, exc_val, exc_tb))  # type: ignore[arg-type]
        return True


def raises(
    expected_exception: Any = None,
    func: Callable[..., object] | None = None,
    *args: Any,
    **kwargs: Any,
) -> Any:
    """Assert that a code block (context manager form, returns a
    ``RaisesExc``) or a function call (returns an ``ExceptionInfo``) raises
    an exception of the given type(s), optionally matching ``match`` and
    ``check``."""
    __tracebackhide__ = True
    if func is None and not args:
        if set(kwargs) - {"match", "check", "expected_exception"}:
            msg = "Unexpected keyword arguments passed to pytest.raises: "
            msg += ", ".join(sorted(kwargs))
            msg += "\nUse context-manager form instead?"
            raise TypeError(msg)
        if expected_exception is None:
            return RaisesExc(**kwargs)
        return RaisesExc(expected_exception, **kwargs)
    if not expected_exception:
        raise ValueError(
            "Expected an exception type or a tuple of exception types, but got "
            f"`{expected_exception!r}`. Raising exceptions is already understood as failing "
            "the test, so you don't need any special code to say 'this should never raise an "
            "exception'."
        )
    if not callable(func):
        raise TypeError(f"{func!r} object (type: {type(func)}) must be callable")
    with RaisesExc(expected_exception) as excinfo:
        func(*args, **kwargs)
    try:
        return excinfo
    finally:
        del excinfo


raises.Exception = Failed  # type: ignore[attr-defined]


# ------------------------------------------------------------------ warnings


class WarningsRecorder(warnings.catch_warnings):
    """A context manager recording the warnings raised inside it: the type
    of the ``recwarn`` fixture."""

    def __init__(self, *, _ispytest: bool = False) -> None:
        super().__init__(record=True)
        self._entered = False
        self._list: list[warnings.WarningMessage] = []

    @property
    def list(self) -> list[warnings.WarningMessage]:
        """The list of recorded warnings."""
        return self._list

    def __getitem__(self, i: int) -> warnings.WarningMessage:
        return self._list[i]

    def __iter__(self) -> Iterator[warnings.WarningMessage]:
        return iter(self._list)

    def __len__(self) -> int:
        return len(self._list)

    def pop(self, cls: type[Warning] = Warning) -> warnings.WarningMessage:
        """Pop the first recorded warning of exactly ``cls``, else the first
        of a subclass that no other match is a subclass of; raise
        ``AssertionError`` if none matches."""
        best: int | None = None
        for i, w in enumerate(self._list):
            if w.category == cls:
                return self._list.pop(i)
            if issubclass(w.category, cls) and (
                best is None or not issubclass(w.category, self._list[best].category)
            ):
                best = i
        if best is not None:
            return self._list.pop(best)
        __tracebackhide__ = True
        raise AssertionError(f"{cls!r} not found in warning list")

    def clear(self) -> None:
        """Clear the list of recorded warnings."""
        self._list[:] = []

    def __enter__(self) -> Self:  # type: ignore[override]
        if self._entered:
            __tracebackhide__ = True
            raise RuntimeError(f"Cannot enter {self!r} twice")
        recorded = super().__enter__()
        self._list = recorded  # type: ignore[assignment]
        warnings.simplefilter("always")
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: types.TracebackType | None,
    ) -> None:
        if not self._entered:
            __tracebackhide__ = True
            raise RuntimeError(f"Cannot exit {self!r} without entering first")
        super().__exit__(exc_type, exc_val, exc_tb)
        # catch_warnings doesn't reset it: reusable, as pytest's.
        self._entered = False


class WarningsChecker(WarningsRecorder):
    """What ``pytest.warns`` returns: fails unless a warning of the
    expected class (matching ``match_expr``) was raised inside it, and
    re-emits the others."""

    def __init__(
        self,
        expected_warning: type[Warning] | tuple[type[Warning], ...] = Warning,
        match_expr: str | re.Pattern[str] | None = None,
        *,
        _ispytest: bool = False,
    ) -> None:
        super().__init__(_ispytest=True)
        msg = "exceptions must be derived from Warning, not %s"
        if isinstance(expected_warning, tuple):
            for exc in expected_warning:
                if not issubclass(exc, Warning):
                    raise TypeError(msg % type(exc))
            expected = expected_warning
        elif isinstance(expected_warning, type) and issubclass(expected_warning, Warning):
            expected = (expected_warning,)
        else:
            raise TypeError(msg % type(expected_warning))
        self.expected_warning = expected
        self.match_expr = match_expr

    def matches(self, warning: warnings.WarningMessage) -> bool:
        return issubclass(warning.category, self.expected_warning) and bool(
            self.match_expr is None or re.search(self.match_expr, str(warning.message))
        )

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: types.TracebackType | None,
    ) -> None:
        super().__exit__(exc_type, exc_val, exc_tb)
        __tracebackhide__ = True
        # Control flow (skip, fail, exit, Ctrl-C) propagates unchecked.
        if exc_val is not None and (
            not isinstance(exc_val, Exception) or isinstance(exc_val, Exit)
        ):
            return

        def found_str() -> str:
            return pprint.pformat([record.message for record in self], indent=2)

        try:
            if not any(issubclass(w.category, self.expected_warning) for w in self):
                fail(
                    f"DID NOT WARN. No warnings of type {self.expected_warning} were emitted.\n"
                    f" Emitted warnings: {found_str()}."
                )
            elif not any(self.matches(w) for w in self):
                escape_hint = ""
                if isinstance(self.match_expr, str) and any(
                    self.match_expr == str(w.message)
                    for w in self
                    if issubclass(w.category, self.expected_warning)
                ):
                    escape_hint = "\n Did you mean to `re.escape()` the regex?"
                fail(
                    f"Regex pattern did not match any of the {len(self)} warnings emitted.\n"
                    f" Regex: {self.match_expr!r}\n"
                    f" Emitted warnings: {found_str()}.{escape_hint}"
                )
        finally:
            # Every warning that didn't match reaches the outer filters.
            for w in self:
                if not self.matches(w):
                    warnings.warn_explicit(
                        message=w.message,
                        category=w.category,
                        filename=w.filename,
                        lineno=w.lineno,
                        module=w.__module__,
                        source=w.source,
                    )
            for w in self:
                if type(w.message) is not UserWarning or not w.message.args:
                    continue
                first = w.message.args[0]
                if isinstance(first, str):
                    continue
                raise TypeError(
                    f"Warning must be str or Warning, got {first!r} (type {type(first).__name__})"
                )


def warns(
    expected_warning: type[Warning] | tuple[type[Warning], ...] = Warning,
    func: Callable[..., object] | None = None,
    *args: Any,
    **kwargs: Any,
) -> Any:
    """Assert that code raises a warning of the given class(es): as a
    context manager (``match=`` checks the message), or around a call,
    returning its result."""
    __tracebackhide__ = True
    if func is None and not args:
        match = kwargs.pop("match", None)
        if kwargs:
            argnames = ", ".join(sorted(kwargs))
            raise TypeError(
                f"Unexpected keyword arguments passed to pytest.warns: {argnames}"
                "\nUse context-manager form instead?"
            )
        return WarningsChecker(expected_warning, match_expr=match, _ispytest=True)
    if not callable(func):
        raise TypeError(f"{func!r} object (type: {type(func)}) must be callable")
    with WarningsChecker(expected_warning, _ispytest=True):
        return func(*args, **kwargs)


def deprecated_call(func: Callable[..., Any] | None = None, *args: Any, **kwargs: Any) -> Any:
    """Assert that code produces a ``DeprecationWarning``,
    ``PendingDeprecationWarning`` or ``FutureWarning``."""
    __tracebackhide__ = True
    dep_warnings = (DeprecationWarning, PendingDeprecationWarning, FutureWarning)
    if func is None:
        return warns(dep_warnings, *args, **kwargs)
    with warns(dep_warnings):
        return func(*args, **kwargs)


# ------------------------------------------------------------------ approx

_TOLERANCE_PLAIN_LOW = 1e-3
_TOLERANCE_PLAIN_HIGH = 1e3


def _compare_approx(
    full_object: object,
    message_data: Sequence[tuple[str, str, str]],
    number_of_elements: int,
    different_ids: Sequence[object],
    max_abs_diff: float,
    max_rel_diff: float,
) -> list[str]:
    message_list = list(message_data)
    message_list.insert(0, ("Index", "Obtained", "Expected"))
    sizes = [0, 0, 0]
    for index, obtained, expected in message_list:
        sizes[0] = max(sizes[0], len(index))
        sizes[1] = max(sizes[1], len(obtained))
        sizes[2] = max(sizes[2], len(expected))
    return [
        f"comparison failed. Mismatched elements: {len(different_ids)} / {number_of_elements}:",
        f"Max absolute difference: {max_abs_diff}",
        f"Max relative difference: {max_rel_diff}",
    ] + [
        f"{indexes:<{sizes[0]}} | {obtained:<{sizes[1]}} | {expected:<{sizes[2]}}"
        for indexes, obtained, expected in message_list
    ]


class ApproxBase:
    """Shared utilities for approximate comparisons between numbers or
    sequences of numbers."""

    # Tell numpy to use our `__eq__` operator instead of its.
    __array_ufunc__ = None
    __array_priority__ = 100

    def __init__(
        self, expected: Any, rel: Any = None, abs: Any = None, nan_ok: bool = False
    ) -> None:
        __tracebackhide__ = True
        self.expected = expected
        self.abs = abs
        self.rel = rel
        self.nan_ok = nan_ok
        self._check_type()

    def __repr__(self) -> str:
        raise NotImplementedError

    def _repr_compare(self, other_side: Any) -> list[str]:
        return ["comparison failed", f"Obtained: {other_side}", f"Expected: {self}"]

    def __eq__(self, actual: object) -> bool:
        return all(a == self._approx_scalar(x) for a, x in self._yield_comparisons(actual))

    def __bool__(self) -> bool:
        __tracebackhide__ = True
        raise AssertionError(
            "approx() is not supported in a boolean context.\n"
            "Did you mean: `assert a == approx(b)`?"
        )

    __hash__ = None  # type: ignore[assignment]

    def __ne__(self, actual: object) -> bool:
        return not (actual == self)

    def _approx_scalar(self, x: Any) -> ApproxBase:
        if isinstance(x, Decimal):
            return ApproxDecimal(x, rel=self.rel, abs=self.abs, nan_ok=self.nan_ok)
        if isinstance(x, datetime | timedelta):
            return ApproxTimedelta(x, rel=self.rel, abs=self.abs, nan_ok=self.nan_ok)
        return ApproxScalar(x, rel=self.rel, abs=self.abs, nan_ok=self.nan_ok)

    def _yield_comparisons(self, actual: Any) -> Iterable[tuple[Any, Any]]:
        raise NotImplementedError

    def _check_type(self) -> None:
        """Raise a TypeError if the expected value is not a valid type."""


def _recursive_sequence_map(f: Callable[[Any], Any], x: Any) -> Any:
    if isinstance(x, list | tuple):
        return type(x)(_recursive_sequence_map(f, xi) for xi in x)
    if _is_sequence_like(x):
        return [_recursive_sequence_map(f, xi) for xi in x]
    return f(x)


def _numpy() -> Any:
    """numpy, when the suite imported it (aye never imports it)."""
    return sys.modules.get("numpy")


class ApproxNumpy(ApproxBase):
    """Approximate comparisons where the expected value is a numpy array."""

    __hash__ = None  # type: ignore[assignment]

    def __repr__(self) -> str:
        list_scalars = _recursive_sequence_map(self._approx_scalar, self.expected.tolist())
        return f"approx({list_scalars!r})"

    def _repr_compare(self, other_side: Any) -> list[str]:
        def value_at(nested: Any, nd_index: tuple[Any, ...]) -> Any:
            value = nested
            for i in nd_index:
                value = value[i]
            return value

        shape = self.expected.shape
        approx_side = _recursive_sequence_map(self._approx_scalar, self.expected.tolist())
        other = _as_numpy_array(other_side)
        if other is None or shape != other.shape:
            return [
                "Impossible to compare arrays with different shapes.",
                f"Shapes: {shape} and {getattr(other, 'shape', None)}",
            ]
        max_abs_diff = -math.inf
        max_rel_diff = -math.inf
        different_ids = []
        for index in itertools.product(*(range(i) for i in shape)):
            approx_value = value_at(approx_side, index)
            other_value = value_at(other, index)
            if approx_value != other_value:
                abs_diff = abs(approx_value.expected - other_value)
                max_abs_diff = max(max_abs_diff, abs_diff)
                if other_value == 0.0:
                    max_rel_diff = math.inf
                else:
                    max_rel_diff = max(max_rel_diff, abs_diff / abs(other_value))
                different_ids.append(index)
        message_data = [
            (str(index), str(value_at(other, index)), str(value_at(approx_side, index)))
            for index in different_ids
        ]
        return _compare_approx(
            self.expected,
            message_data,
            self.expected.size,
            different_ids,
            max_abs_diff,
            max_rel_diff,
        )

    def __eq__(self, actual: object) -> bool:
        np = _numpy()
        if not np.isscalar(actual):
            try:
                actual = np.asarray(actual)
            except Exception as e:
                raise TypeError(f"cannot compare '{actual}' to numpy.ndarray") from e
        if not np.isscalar(actual) and actual.shape != self.expected.shape:  # type: ignore[attr-defined]
            return False
        return super().__eq__(actual)

    def _yield_comparisons(self, actual: Any) -> Iterator[tuple[Any, Any]]:
        np = _numpy()
        if np.isscalar(actual):
            for i in np.ndindex(self.expected.shape):
                yield actual, self.expected[i].item()
        else:
            for i in np.ndindex(self.expected.shape):
                yield actual[i].item(), self.expected[i].item()


class ApproxMapping(ApproxBase):
    """Approximate comparisons where the expected value is a mapping with
    numeric values (the keys can be anything)."""

    __hash__ = None  # type: ignore[assignment]

    def __repr__(self) -> str:
        return f"approx({ ({k: self._approx_scalar(v) for k, v in self.expected.items()})!r})"

    def _repr_compare(self, other_side: Any) -> list[str]:
        if len(self.expected) != len(other_side):
            return [
                "Impossible to compare mappings with different sizes.",
                f"Lengths: {len(self.expected)} and {len(other_side)}",
            ]
        if self.expected.keys() != other_side.keys():
            return [
                "comparison failed.",
                (
                    f"Mappings has different keys: expected {self.expected.keys()} "
                    f"but got {other_side.keys()}"
                ),
            ]
        approx_side = {k: self._approx_scalar(v) for k, v in self.expected.items()}
        max_abs_diff = -math.inf
        max_rel_diff = -math.inf
        different_ids = []
        for key, approx_value in approx_side.items():
            other_value = other_side[key]
            if approx_value != other_value:
                if approx_value.expected is not None and other_value is not None:
                    try:
                        max_abs_diff = max(max_abs_diff, abs(approx_value.expected - other_value))
                        if approx_value.expected == 0.0:
                            max_rel_diff = math.inf
                        else:
                            max_rel_diff = max(
                                max_rel_diff,
                                abs((approx_value.expected - other_value) / approx_value.expected),
                            )
                    except ZeroDivisionError:
                        pass
                different_ids.append(key)
        message_data = [
            (str(key), str(other_side[key]), str(approx_side[key])) for key in different_ids
        ]
        return _compare_approx(
            self.expected, message_data, len(approx_side), different_ids, max_abs_diff, max_rel_diff
        )

    def __eq__(self, actual: object) -> bool:
        try:
            if set(actual.keys()) != set(self.expected.keys()):  # type: ignore[attr-defined]
                return False
        except AttributeError:
            return False
        return super().__eq__(actual)

    def _yield_comparisons(self, actual: Any) -> Iterator[tuple[Any, Any]]:
        for k in self.expected:
            yield actual[k], self.expected[k]

    def _check_type(self) -> None:
        __tracebackhide__ = True
        for key, value in self.expected.items():
            if isinstance(value, type(self.expected)):
                msg = (
                    "pytest.approx() does not support nested dictionaries: key={!r} value={!r}\n"
                    "  full mapping={}"
                )
                raise TypeError(msg.format(key, value, pprint.pformat(self.expected)))


class ApproxSequenceLike(ApproxBase):
    """Approximate comparisons where the expected value is a sequence of numbers."""

    __hash__ = None  # type: ignore[assignment]

    def __repr__(self) -> str:
        seq_type = type(self.expected)
        if seq_type not in (tuple, list):
            seq_type = list
        return f"approx({seq_type(self._approx_scalar(x) for x in self.expected)!r})"

    def _repr_compare(self, other_side: Any) -> list[str]:
        if len(self.expected) != len(other_side):
            return [
                "Impossible to compare lists with different sizes.",
                f"Lengths: {len(self.expected)} and {len(other_side)}",
            ]
        approx_side = _recursive_sequence_map(self._approx_scalar, self.expected)
        max_abs_diff = -math.inf
        max_rel_diff = -math.inf
        different_ids = []
        for i, (approx_value, other_value) in enumerate(zip(approx_side, other_side, strict=True)):
            if approx_value != other_value:
                try:
                    abs_diff = abs(approx_value.expected - other_value)
                    max_abs_diff = max(max_abs_diff, abs_diff)
                except TypeError:
                    pass  # not numbers: no differences to compute
                else:
                    if other_value == 0.0:
                        max_rel_diff = math.inf
                    else:
                        max_rel_diff = max(max_rel_diff, abs_diff / abs(other_value))
                different_ids.append(i)
        message_data = [(str(i), str(other_side[i]), str(approx_side[i])) for i in different_ids]
        return _compare_approx(
            self.expected, message_data, len(approx_side), different_ids, max_abs_diff, max_rel_diff
        )

    def __eq__(self, actual: object) -> bool:
        try:
            if len(actual) != len(self.expected):  # type: ignore[arg-type]
                return False
        except TypeError:
            return False
        return super().__eq__(actual)

    def _yield_comparisons(self, actual: Any) -> Iterable[tuple[Any, Any]]:
        return zip(actual, self.expected, strict=True)

    def _check_type(self) -> None:
        __tracebackhide__ = True
        for index, x in enumerate(self.expected):
            if isinstance(x, type(self.expected)):
                msg = (
                    "pytest.approx() does not support nested data structures: {!r} at index {}\n"
                    "  full sequence: {}"
                )
                raise TypeError(msg.format(x, index, pprint.pformat(self.expected)))


def _is_bool(value: Any) -> bool:
    """A bool, or numpy's (when numpy is loaded)."""
    if isinstance(value, bool):
        return True
    np = _numpy()
    return bool(np is not None and isinstance(value, np.bool_))


class ApproxScalar(ApproxBase):
    """Approximate comparisons where the expected value is a single number."""

    DEFAULT_ABSOLUTE_TOLERANCE: float | Decimal = 1e-12
    DEFAULT_RELATIVE_TOLERANCE: float | Decimal = 1e-6

    def __repr__(self) -> str:
        """``1.0 ± 1.0e-06``, ``(3+4j) ± 5.0e-06 ∠ ±180°``; no tolerance for
        non-numbers and infinities."""
        if (
            isinstance(self.expected, bool)
            or not isinstance(self.expected, Complex | Decimal)
            or math.isinf(abs(self.expected) or isinstance(self.expected, bool))
        ):
            return str(self.expected)
        try:
            if _TOLERANCE_PLAIN_LOW <= self.tolerance < _TOLERANCE_PLAIN_HIGH:
                vetted_tolerance = f"{self.tolerance:n}"
            else:
                vetted_tolerance = f"{self.tolerance:.1e}"
            if (
                isinstance(self.expected, Complex)
                and self.expected.imag
                and not math.isinf(self.tolerance)
            ):
                vetted_tolerance += " ∠ ±180°"
        except ValueError:
            vetted_tolerance = "???"
        return f"{self.expected} ± {vetted_tolerance}"

    def __eq__(self, actual: object) -> bool:  # noqa: PLR0911 - pytest's order of checks
        """Whether ``actual`` is equal to the expected value within the
        tolerance."""
        asarray = _as_numpy_array(actual)
        if asarray is not None:
            return all(self.__eq__(a) for a in asarray.flat)
        if _is_bool(self.expected) and not _is_bool(actual):
            return False
        if actual == self.expected:
            return True
        # Non-numbers (and bools) only compare equal exactly.
        if _is_bool(self.expected) or not (
            isinstance(self.expected, Complex | Decimal) and isinstance(actual, Complex | Decimal)
        ):
            return False
        if math.isnan(abs(self.expected)):
            return self.nan_ok and math.isnan(abs(actual))
        if math.isinf(abs(self.expected)):
            return False
        result: bool = abs(self.expected - actual) <= self.tolerance
        return result

    __hash__ = None  # type: ignore[assignment]

    @property
    def tolerance(self) -> Any:
        """The larger of the absolute tolerance and the relative one times
        the expected value (only the absolute one when only it was given)."""
        absolute = self.abs if self.abs is not None else self.DEFAULT_ABSOLUTE_TOLERANCE
        if absolute < 0:
            raise ValueError(f"absolute tolerance can't be negative: {absolute}")
        if math.isnan(absolute):
            raise ValueError("absolute tolerance can't be NaN.")
        if self.rel is None and self.abs is not None:
            return absolute
        relative = (self.rel if self.rel is not None else self.DEFAULT_RELATIVE_TOLERANCE) * abs(
            self.expected
        )
        if relative < 0:
            raise ValueError(f"relative tolerance can't be negative: {relative}")
        if math.isnan(relative):
            raise ValueError("relative tolerance can't be NaN.")
        return max(relative, absolute)


class ApproxDecimal(ApproxScalar):
    """Approximate comparisons where the expected value is a Decimal."""

    DEFAULT_ABSOLUTE_TOLERANCE = Decimal("1e-12")
    DEFAULT_RELATIVE_TOLERANCE = Decimal("1e-6")

    def __repr__(self) -> str:
        rel = Decimal.from_float(self.rel) if isinstance(self.rel, float) else self.rel
        abs_ = Decimal.from_float(self.abs) if isinstance(self.abs, float) else self.abs
        tol_str = "???"
        if rel is not None and Decimal("1e-3") <= rel <= Decimal("1e3"):
            tol_str = f"{rel:.1e}"
        elif abs_ is not None:
            tol_str = f"{abs_:.1e}"
        return f"{self.expected} ± {tol_str}"


class ApproxTimedelta(ApproxBase):
    """Approximate comparisons where the expected value is a datetime or a
    timedelta: ``abs`` a timedelta, or ``rel`` a number (timedelta only)."""

    __hash__ = None  # type: ignore[assignment]

    def __init__(
        self, expected: Any, rel: Any = None, abs: Any = None, nan_ok: bool = False
    ) -> None:
        __tracebackhide__ = True
        if isinstance(expected, datetime) and rel is not None:
            raise TypeError(
                "pytest.approx() does not support relative tolerance for "
                "datetime comparisons. Use abs=timedelta(...) instead."
            )
        if nan_ok:
            raise TypeError(
                "pytest.approx() does not support nan_ok for datetime/timedelta comparisons."
            )
        if abs is None and rel is None:
            raise TypeError(
                "pytest.approx() requires an explicit tolerance for "
                "datetime/timedelta comparisons: "
                "e.g. approx(expected, abs=timedelta(seconds=1)) "
                "or approx(expected, rel=0.01)"
            )
        if abs is not None and not isinstance(abs, timedelta):
            raise TypeError(
                "absolute tolerance for datetime/timedelta must be a "
                f"timedelta, got {type(abs).__name__}"
            )
        if abs is not None and abs < timedelta(0):
            raise ValueError(f"absolute tolerance can't be negative: {abs}")
        if rel is not None:
            if not isinstance(rel, int | float):
                raise TypeError(
                    f"relative tolerance for timedelta must be a number, got {type(rel).__name__}"
                )
            if rel < 0:
                raise ValueError(f"relative tolerance can't be negative: {rel}")
            if math.isnan(rel):
                raise ValueError("relative tolerance can't be NaN.")
        rel_tolerance = rel * builtins.abs(expected) if rel is not None else None
        if abs is not None and rel_tolerance is not None:
            tolerance = max(abs, rel_tolerance)
        else:
            tolerance = abs if abs is not None else rel_tolerance
        super().__init__(expected, rel=rel, abs=tolerance, nan_ok=False)

    def __repr__(self) -> str:
        return f"{self.expected} ± {self.abs}"

    def __eq__(self, actual: object) -> bool:
        try:
            return bool(builtins.abs(self.expected - actual) <= self.abs)
        except (TypeError, OverflowError):
            return False

    def _yield_comparisons(self, actual: Any) -> Iterator[tuple[Any, Any]]:
        yield actual, self.expected

    def _repr_compare(self, other_side: Any) -> list[str]:
        try:
            abs_diff: Any = builtins.abs(self.expected - other_side)
        except (TypeError, OverflowError):
            abs_diff = "N/A"
        return [
            "comparison failed",
            f"Obtained: {other_side}",
            f"Expected: {self.expected} ± {self.abs}",
            f"Absolute difference: {abs_diff}",
            f"Tolerance: {self.abs}",
        ]


def approx(expected: Any, rel: Any = None, abs: Any = None, nan_ok: bool = False) -> ApproxBase:
    """Assert that two numbers (or two ordered sequences of numbers, or the
    values of two mappings) are equal within a tolerance: ``rel`` (1e-6 by
    default) of the expected value, or ``abs`` (1e-12 by default)."""
    __tracebackhide__ = True
    cls: type[ApproxBase]
    if isinstance(expected, Decimal):
        cls = ApproxDecimal
    elif isinstance(expected, Mapping):
        cls = ApproxMapping
    elif (np_array := _as_numpy_array(expected)) is not None:
        expected = np_array
        cls = ApproxNumpy
    elif _is_sequence_like(expected):
        cls = ApproxSequenceLike
    elif isinstance(expected, Collection) and not isinstance(expected, str | bytes):
        msg = f"pytest.approx() only supports ordered sequences, but got: {expected!r}"
        raise TypeError(msg)
    elif isinstance(expected, datetime | timedelta):
        cls = ApproxTimedelta
    else:
        cls = ApproxScalar
    return cls(expected, rel, abs, nan_ok)


def _is_sequence_like(expected: object) -> bool:
    return (
        hasattr(expected, "__getitem__")
        and isinstance(expected, Sized)
        and not isinstance(expected, str | bytes)
    )


def _as_numpy_array(obj: object) -> Any:
    """``obj`` as an ndarray when numpy is loaded and ``obj`` converts
    implicitly, else ``None``."""
    np = _numpy()
    if np is not None:
        # Numpy scalars have __array__ too: no infinite recursion.
        if np.isscalar(obj):
            return None
        if isinstance(obj, np.ndarray):
            return obj
        if hasattr(obj, "__array__") or hasattr(obj, "__array_interface__"):
            return np.asarray(obj)
    return None


# ------------------------------------------------------------------ monkeypatch


def _resolve(name: str) -> object:
    """Import the longest importable prefix of a dotted name, then get the
    rest as attributes."""
    parts = name.split(".")
    used = parts.pop(0)
    found: object = importlib.import_module(used)
    for part in parts:
        used += "." + part
        try:
            found = getattr(found, part)
        except AttributeError:
            pass
        else:
            continue
        # A module whose import fails says which one it failed on.
        try:
            importlib.import_module(used)
        except ImportError as ex:
            expected = str(ex).split()[-1]
            if expected == used:
                raise
            raise ImportError(f"import error in {used}: {ex}") from ex
        found = _annotated_getattr(found, part, used)
    return found


def _annotated_getattr(obj: object, name: str, ann: str) -> object:
    try:
        return getattr(obj, name)
    except AttributeError as e:
        raise AttributeError(
            f"{type(obj).__name__!r} object at {ann} has no attribute {name!r}"
        ) from e


def _derive_importpath(import_path: str, raising: bool) -> tuple[str, object]:
    if not isinstance(import_path, str) or "." not in import_path:
        raise TypeError(f"must be absolute import path string, not {import_path!r}")
    module, attr = import_path.rsplit(".", 1)
    target = _resolve(module)
    if raising:
        _annotated_getattr(target, attr, ann=module)
    return attr, target


class MonkeyPatch:
    """Helper to conveniently monkeypatch attributes, items, environment
    variables and ``sys.path``, undone by ``undo()``: the ``monkeypatch``
    fixture's type, also usable directly (``MonkeyPatch.context()``)."""

    def __init__(self) -> None:
        self._setattr: list[tuple[object, str, object]] = []
        self._setitem: list[tuple[Any, object, object]] = []
        self._cwd: str | None = None
        self._savesyspath: list[str] | None = None

    @classmethod
    @contextmanager
    def context(cls) -> Iterator[MonkeyPatch]:
        """A new ``MonkeyPatch`` whose patches are undone when the block ends."""
        m = cls()
        try:
            yield m
        finally:
            m.undo()

    def setattr(self, target: Any, name: Any, value: Any = NOTSET, raising: bool = True) -> None:
        """Set an attribute, remembering the old value: ``setattr(obj, name,
        value)``, or ``setattr("dotted.path.name", value)``. With
        ``raising``, the attribute must exist."""
        __tracebackhide__ = True
        if value is NOTSET:
            if not isinstance(target, str):
                raise TypeError(
                    "use setattr(target, name, value) or "
                    "setattr(target, value) with target being a dotted "
                    "import string"
                )
            value = name
            name, target = _derive_importpath(target, raising)
        elif not isinstance(name, str):
            raise TypeError(
                "use setattr(target, name, value) with name being a string or "
                "setattr(target, value) with target being a dotted "
                "import string"
            )
        oldval = getattr(target, name, NOTSET)
        if raising and oldval is NOTSET:
            raise AttributeError(f"{target!r} has no attribute {name!r}")
        # For a class, keep the descriptor itself (a staticmethod, say).
        if inspect.isclass(target):
            oldval = target.__dict__.get(name, NOTSET)
        setattr(target, name, value)
        self._setattr.append((target, name, oldval))

    def delattr(self, target: Any, name: Any = NOTSET, raising: bool = True) -> None:
        """Delete an attribute (``delattr(obj, name)`` or
        ``delattr("dotted.path.name")``). With ``raising``, it must exist."""
        __tracebackhide__ = True
        if name is NOTSET:
            if not isinstance(target, str):
                raise TypeError(
                    "use delattr(target, name) or "
                    "delattr(target) with target being a dotted "
                    "import string"
                )
            name, target = _derive_importpath(target, raising)
        if not hasattr(target, name):
            if raising:
                raise AttributeError(name)
        else:
            oldval = getattr(target, name, NOTSET)
            if inspect.isclass(target):
                oldval = target.__dict__.get(name, NOTSET)
            self._setattr.append((target, name, oldval))
            delattr(target, name)

    def setitem(self, dic: Any, name: Any, value: Any) -> None:
        """Set dictionary entry ``name`` to value."""
        self._setitem.append((dic, name, dic.get(name, NOTSET)))
        dic[name] = value

    def delitem(self, dic: Any, name: Any, raising: bool = True) -> None:
        """Delete ``name`` from a dict; with ``raising``, it must exist."""
        if name not in dic:
            if raising:
                raise KeyError(name)
        else:
            self._setitem.append((dic, name, dic.get(name, NOTSET)))
            del dic[name]

    def setenv(self, name: str, value: str, prepend: str | None = None) -> None:
        """Set an environment variable; with ``prepend`` (a separator) and
        the variable set, ``value + prepend + old``."""
        if not isinstance(value, str):
            warnings.warn(  # type: ignore[unreachable]
                PytestWarning(
                    f"Value of environment variable {name} type should be str, but got "
                    f"{value!r} (type: {type(value).__name__}); converted to str implicitly"
                ),
                stacklevel=2,
            )
            value = str(value)
        if prepend and name in os.environ:
            value = value + prepend + os.environ[name]
        self.setitem(os.environ, name, value)

    def delenv(self, name: str, raising: bool = True) -> None:
        """Delete an environment variable; with ``raising``, it must exist."""
        environ: MutableMapping[str, str] = os.environ
        self.delitem(environ, name, raising=raising)

    def syspath_prepend(self, path: Any) -> None:
        """Prepend ``path`` to ``sys.path``."""
        if self._savesyspath is None:
            self._savesyspath = sys.path[:]
        sys.path.insert(0, str(path))
        # Legacy namespace packages, when the suite uses pkg_resources.
        fixup = getattr(sys.modules.get("pkg_resources"), "fixup_namespace_packages", None)
        if fixup is not None:
            fixup(str(path))
        importlib.invalidate_caches()

    def chdir(self, path: str | os.PathLike[str]) -> None:
        """Change the current working directory, remembering the first one."""
        if self._cwd is None:
            self._cwd = os.getcwd()
        os.chdir(path)

    def undo(self) -> None:
        """Undo every change, last first: attributes, then items, then
        ``sys.path``, then the working directory. A second call does nothing
        unless more was patched since."""
        for obj, name, value in reversed(self._setattr):
            if value is not NOTSET:
                setattr(obj, name, value)
            else:
                delattr(obj, name)
        self._setattr[:] = []
        for dictionary, key, value in reversed(self._setitem):
            if value is NOTSET:
                try:
                    del dictionary[key]
                except KeyError:
                    pass  # already deleted: the state wanted
            else:
                dictionary[key] = value
        self._setitem[:] = []
        if self._savesyspath is not None:
            sys.path[:] = self._savesyspath
            self._savesyspath = None
        if self._cwd is not None:
            os.chdir(self._cwd)
            self._cwd = None


# ------------------------------------------------------------------ _pytest


class AssertionRewritingHook:
    """pytest's assertion rewriting import hook, which aye's module doesn't
    provide (aye rewrites the suite's asserts itself)."""

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        raise UsageError(_not_provided("_pytest.assertion.rewrite.AssertionRewritingHook"))


# The ``_pytest`` modules suites import from, and the names each has here.
_PRIVATE_MODULES: dict[str, tuple[str, ...]] = {
    "_pytest": ("__version__", "version_tuple"),
    "_pytest._code": ("ExceptionInfo", "Code", "Frame", "Traceback", "TracebackEntry"),
    "_pytest._code.code": ("ExceptionInfo", "Code", "Frame", "Traceback", "TracebackEntry"),
    "_pytest.assertion": ("register_assert_rewrite",),
    "_pytest.assertion.rewrite": ("AssertionRewritingHook",),
    "_pytest.cacheprovider": ("Cache",),
    "_pytest.capture": ("CaptureFixture",),
    "_pytest.compat": ("NOTSET",),
    "_pytest.config": (
        "Config",
        "ExitCode",
        "UsageError",
        "hookimpl",
        "hookspec",
        "PytestPluginManager",
        "Parser",
        "main",
        "console_main",
    ),
    "_pytest.config.argparsing": ("Parser", "OptionGroup"),
    "_pytest.config.exceptions": ("UsageError",),
    "_pytest.doctest": ("DoctestItem",),
    "_pytest.fixtures": (
        "FixtureRequest",
        "TopRequest",
        "SubRequest",
        "FixtureLookupError",
        "fixture",
        "yield_fixture",
    ),
    "_pytest.logging": ("LogCaptureFixture",),
    "_pytest.main": ("Session", "Dir"),
    "_pytest.mark": (
        "Mark",
        "MarkDecorator",
        "MarkGenerator",
        "MARK_GEN",
        "ParameterSet",
        "HIDDEN_PARAM",
        "param",
    ),
    "_pytest.mark.structures": (
        "Mark",
        "MarkDecorator",
        "MarkGenerator",
        "MARK_GEN",
        "ParameterSet",
        "HIDDEN_PARAM",
        "param",
    ),
    "_pytest.monkeypatch": ("MonkeyPatch",),
    "_pytest.nodes": ("Node", "Item", "Collector", "FSCollector", "File", "Directory"),
    "_pytest.outcomes": (
        "OutcomeException",
        "Skipped",
        "Failed",
        "XFailed",
        "Exit",
        "skip",
        "fail",
        "xfail",
        "exit",
        "importorskip",
        "TEST_OUTCOME",
    ),
    "_pytest.python": (
        "Function",
        "Class",
        "Module",
        "Package",
        "Metafunc",
        "PyCollector",
        "FunctionDefinition",
    ),
    "_pytest.python_api": ("approx",),
    "_pytest.raises": ("raises", "RaisesExc"),
    "_pytest.recwarn": ("WarningsRecorder", "WarningsChecker", "warns", "deprecated_call"),
    "_pytest.reports": ("TestReport", "CollectReport"),
    "_pytest.runner": ("CallInfo",),
    "_pytest.stash": ("Stash", "StashKey"),
    "_pytest.tmpdir": ("TempPathFactory",),
    "_pytest.warning_types": (
        "PytestWarning",
        "PytestAssertRewriteWarning",
        "PytestCacheWarning",
        "PytestCollectionWarning",
        "PytestConfigWarning",
        "PytestDeprecationWarning",
        "PytestExperimentalApiWarning",
        "PytestFDWarning",
        "PytestRemovedIn9Warning",
        "PytestRemovedIn10Warning",
        "PytestReturnNotNoneWarning",
        "PytestUnhandledThreadExceptionWarning",
        "PytestUnknownMarkWarning",
        "PytestUnraisableExceptionWarning",
    ),
}
# Names the ``_pytest`` modules have under another name here.
_PRIVATE_ALIASES = {
    "get_unpacked_marks": "_get_unpacked_marks",
    "normalize_mark_list": "_normalize_mark_list",
    "store_mark": "_store_mark",
    "istestfunc": "_istestfunc",
    "getfixturemarker": "_fixture_spec",
}
_PRIVATE_EXTRAS = {
    "_pytest.mark.structures": (
        "get_unpacked_marks",
        "normalize_mark_list",
        "store_mark",
        "istestfunc",
    ),
    "_pytest.fixtures": ("getfixturemarker",),
}


class _PrivateModules:
    """A ``sys.meta_path`` finder serving ``_pytest`` and its modules as
    aliases of this module's objects; any other ``_pytest`` module raises a
    ``ModuleNotFoundError`` naming it."""

    def __init__(self, namespace: dict[str, Any]) -> None:
        self.namespace = namespace

    def find_spec(self, fullname: str, path: Any = None, target: Any = None) -> Any:
        if fullname != "_pytest" and not fullname.startswith("_pytest."):
            return None
        if fullname not in _PRIVATE_MODULES:
            raise ModuleNotFoundError(
                f"No module named {fullname!r}: {_not_provided(fullname)}", name=fullname
            )
        package = any(name.startswith(fullname + ".") for name in _PRIVATE_MODULES)
        return importlib.machinery.ModuleSpec(fullname, self, is_package=package)

    def create_module(self, spec: Any) -> None:
        return None

    def exec_module(self, module: types.ModuleType) -> None:
        name = module.__name__
        for attribute in _PRIVATE_MODULES[name] + _PRIVATE_EXTRAS.get(name, ()):
            if attribute == "FixtureLookupError":
                value: Any = _fixture_lookup_error()
            else:
                value = self.namespace[_PRIVATE_ALIASES.get(attribute, attribute)]
            setattr(module, attribute, value)

        def missing(attribute: str) -> Any:
            if attribute.startswith("__") and attribute.endswith("__"):
                raise AttributeError(f"module {name!r} has no attribute {attribute!r}")
            raise AttributeError(
                f"module {name!r} has no attribute {attribute!r}: "
                f"{_not_provided(f'{name}.{attribute}')}"
            )

        module.__getattr__ = missing  # type: ignore[method-assign]


def _aye_install_private_modules() -> bool:
    """Serve ``_pytest`` and its modules from this module's objects (first
    on ``sys.meta_path``). An earlier install of this source (another
    ``pytest`` module object) is replaced, with the aliases it served. Does
    nothing, and returns ``False``, when this module's finder is installed
    already or a ``_pytest`` of another origin (a real pytest's) is imported."""
    finders = [f for f in sys.meta_path if type(f).__name__ == _PrivateModules.__name__]
    if any(isinstance(f, _PrivateModules) for f in finders):
        return False
    if finders:
        for name, module in list(sys.modules.items()):
            loader = getattr(getattr(module, "__spec__", None), "loader", None)
            if any(loader is f for f in finders):
                del sys.modules[name]
        sys.meta_path[:] = [f for f in sys.meta_path if not any(f is g for g in finders)]
    if "_pytest" in sys.modules:
        return False
    sys.meta_path.insert(0, _PrivateModules(globals()))
    # As ``import pytest`` imports ``_pytest.outcomes``: adapters that tell
    # pytest's outcomes by ``sys.modules["_pytest.outcomes"]`` (the doctest
    # adapter, for a conftest's ``importorskip``) find this module's.
    importlib.import_module("_pytest.outcomes")
    return True


_DISTRIBUTION = "pytest"


def _normalized_distribution_name(name: str) -> str:
    """A distribution name as PEP 503 normalizes it (``Py_Test`` is
    ``py-test``)."""
    return re.sub(r"[-_.]+", "-", name).lower()


class _Distributions:
    """A ``sys.meta_path`` finder that finds no module and one distribution,
    ``pytest`` at this module's version, so that
    ``importlib.metadata.version("pytest")`` gives the version of the pytest
    that ``import pytest`` gives (this module), with no pytest installed or
    ahead of an installed one."""

    def __init__(self, version: str, location: str) -> None:
        self.version = version
        self.location = location
        self._class: Any = None

    def find_spec(self, fullname: str, path: Any = None, target: Any = None) -> None:
        return None

    def find_distributions(self, context: Any = None) -> Iterator[Any]:
        """The ``pytest`` distribution, when ``context`` asks for every
        distribution (no name) or for pytest."""
        name = getattr(context, "name", None)
        if name is None or _normalized_distribution_name(name) == _DISTRIBUTION:
            yield self._distribution_class()()

    def _distribution_class(self) -> Any:
        """An ``importlib.metadata.Distribution`` with only a ``METADATA``
        file, made on the first search: whoever searches has imported
        ``importlib.metadata`` already, and the worker needn't."""
        if self._class is None:
            from importlib.metadata import Distribution  # noqa: PLC0415 - see above

            metadata = f"Metadata-Version: 2.1\nName: {_DISTRIBUTION}\nVersion: {self.version}\n"
            location = self.location

            class PytestDistribution(Distribution):
                """aye's ``pytest``: a name and a version, no files."""

                def read_text(self, filename: str) -> str | None:
                    return metadata if filename == "METADATA" else None

                def locate_file(self, path: Any) -> PurePath:
                    return PurePath(location, path)

            self._class = PytestDistribution
        return self._class


def _aye_install_distribution() -> bool:
    """Register the ``pytest`` distribution (``_Distributions``, first on
    ``sys.meta_path``, ahead of an installed pytest's). An earlier install of
    this source's finder is replaced, so one is ever registered. Does
    nothing, and returns ``False``, when this module's finder is there
    already."""
    finders = [f for f in sys.meta_path if type(f).__name__ == _Distributions.__name__]
    if any(isinstance(f, _Distributions) for f in finders):
        return False
    sys.meta_path[:] = [f for f in sys.meta_path if not any(f is g for g in finders)]
    sys.meta_path.insert(0, _Distributions(__version__, os.path.dirname(__file__)))
    return True


__all__ = [
    "HIDDEN_PARAM",
    "Cache",
    "CallInfo",
    "CaptureFixture",
    "Class",
    "CollectReport",
    "Collector",
    "Config",
    "Dir",
    "Directory",
    "DoctestItem",
    "ExceptionInfo",
    "ExitCode",
    "File",
    "FixtureLookupError",  # noqa: F822 - served by __getattr__
    "FixtureRequest",
    "Function",
    "Item",
    "LogCaptureFixture",
    "Mark",
    "MarkDecorator",
    "MarkGenerator",
    "Metafunc",
    "Module",
    "MonkeyPatch",
    "OptionGroup",
    "Package",
    "Parser",
    "PytestAssertRewriteWarning",
    "PytestCacheWarning",
    "PytestCollectionWarning",
    "PytestConfigWarning",
    "PytestDeprecationWarning",
    "PytestExperimentalApiWarning",
    "PytestFDWarning",
    "PytestPluginManager",
    "PytestRemovedIn10Warning",
    "PytestReturnNotNoneWarning",
    "PytestUnhandledThreadExceptionWarning",
    "PytestUnknownMarkWarning",
    "PytestUnraisableExceptionWarning",
    "PytestWarning",
    "RaisesExc",
    "ScopeName",
    "Session",
    "Stash",
    "StashKey",
    "TempPathFactory",
    "TestReport",
    "UsageError",
    "WarningsRecorder",
    "__version__",
    "approx",
    "cmdline",
    "console_main",
    "deprecated_call",
    "exit",
    "fail",
    "fixture",
    "hookimpl",
    "hookspec",
    "importorskip",
    "main",
    "mark",
    "param",
    "raises",
    "register_assert_rewrite",
    "set_trace",
    "skip",
    "version_tuple",
    "warns",
    "xfail",
    "yield_fixture",
]

if __name__ == "pytest":
    _aye_install_private_modules()
    _aye_install_distribution()
