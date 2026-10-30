"""aye's engine: what a test is and how it executes.

This module runs inside workers. The runner embeds it in its binary and the
worker installs it under the name ``aye_engine`` before any executor, so every
adapter builds on the same objects:

- ``Outcome`` and ``Phase``: the fixed outcome set and where things happen.
- ``Failure``: a structured failure built from an exception. Its frames
  say where they point (``describe_frames``: each frame's span, and on the
  innermost frame of the project the lines before it and its locals), and
  ``compare`` says how two unequal values differ, entry by entry. Both work
  only once a test failed: a passing test pays nothing for them.
- ``Sink``: where events go. The runner merges every worker's sink into the
  run's log. Every record it writes is valid JSON in valid UTF-8: lone
  surrogates become U+FFFD and non-finite numbers are dropped.
- ``Services``: what the runner provides inside the worker, looked up by name.
- ``Lifetime`` and ``Lifetimes``: shared setup with reverse-order teardown,
  opened and closed as the order of tests requires.
- ``Test``: one test with its setup, body and teardown.
- ``Executor``: an adapter's runtime face. ``EngineExecutor`` runs the loop
  for adapters that map fully onto the engine (unittest, doctest); the pytest
  adapter subclasses ``Executor`` and hands the loop to pytest.
- ``reproduce``: the command that reruns one test: ``aye``, aye's own
  options that shaped the run (``rerun_args``, from the context), the id,
  then ``--`` and the arguments the user gave after it. ``EngineExecutor``
  sets it on every bad outcome.
- ``is_skip``: whether an exception means "skipped": aye's ``Skip``,
  unittest's ``SkipTest`` and pytest's ``skip`` (whose class says it lives in
  ``builtins``), subclasses included.
- ``CaptureIO`` and ``captured``: per-test capture of ``sys.stdout`` and
  ``sys.stderr`` for adapters whose runner provides no ``capture`` service,
  with streams that behave as pytest's do (``.buffer``, ``.encoding``,
  ``fileno()``) and stay readable after a test closes them.

Tests run in the same process and may patch or replace what the engine uses
(``time.perf_counter``, ``os.path``, ``traceback``, ``math``, ``hashlib``).
The engine binds those functions when it is imported, as pytest binds its
clock, and computes paths with them (``Paths``), so a test that replaces
``time.perf_counter`` (with one returning NaN, say) changes no duration and
one that patches ``os.path.abspath`` changes no reported path. ``os_path`` is
``os.path`` as it was then, for adapters: patching ``os.path.join`` changes
none of its functions, ``abspath`` and ``realpath`` included. Building a
failure never raises: what can't be read (a frame, a broken ``__str__``) is
left out. Adapters use ``perf_counter`` too.

It imports only the standard library and runs on Python 3.10 and later.
"""

from __future__ import annotations

import enum
import os
import re
import reprlib
import sys
from collections.abc import Callable, Iterable, Iterator, Mapping, Sequence
from collections.abc import Set as AbstractSet
from contextlib import contextmanager

# Bound at import, so that a test replacing these module attributes (as
# ``mock.patch("os.path.abspath")`` does, or ``time.perf_counter = ...``)
# changes nothing the engine reports.
from hashlib import sha1
from io import BytesIO, TextIOWrapper
from itertools import islice
from linecache import getline
from math import isfinite
from time import perf_counter
from traceback import extract_tb
from traceback import format_exception as _format_exception
from types import (
    BuiltinFunctionType,
    BuiltinMethodType,
    FunctionType,
    MethodType,
    ModuleType,
    SimpleNamespace,
)
from typing import Any, TypeVar

_fspath = os.fspath
_getcwd = os.getcwd
_isabs = os.path.isabs
_join = os.path.join
_normcase = os.path.normcase
_normpath = os.path.normpath
try:  # Windows: what os.path.abspath uses, so a rooted path takes the drive
    from nt import _getfullpathname as _fullpath
except ImportError:
    _fullpath = None
_SEP = os.sep


def _frozen(module: Any) -> Any:
    """A copy of ``module``'s namespace whose functions look their globals
    up in the copy, so that a test patching the module's attributes later
    (``os.path.join = …``) changes none of them, not even those calling the
    patched one (``abspath``, ``relpath``, ``realpath``)."""
    original = vars(module)
    namespace = dict(original)
    for name, value in original.items():
        if isinstance(value, FunctionType) and value.__globals__ is original:
            copy = FunctionType(
                value.__code__, namespace, value.__name__, value.__defaults__, value.__closure__
            )
            copy.__kwdefaults__ = value.__kwdefaults__
            copy.__qualname__ = value.__qualname__
            copy.__doc__ = value.__doc__
            namespace[name] = copy
    return SimpleNamespace(**{k: v for k, v in namespace.items() if not k.startswith("__")})


# ``os.path`` as it was when the engine was imported, for adapters.
os_path = _frozen(os.path)

_OS_PATH = dict(vars(os.path))
_ABSENT = object()


@contextmanager
def unpatched_os_path() -> Iterator[None]:
    """``os.path``'s own functions back in place while the block runs (an
    adapter loading a batch's modules), and what earlier tests left patched
    put back afterwards: a framework imports its tests before running any,
    so no test's leftover patch reaches its loader or the modules it
    imports."""
    live = vars(os.path)
    changed = {
        name: live.get(name, _ABSENT)
        for name, value in _OS_PATH.items()
        if live.get(name, _ABSENT) is not value
    }
    for name in changed:
        setattr(os.path, name, _OS_PATH[name])
    try:
        yield
    finally:
        for name, value in changed.items():
            if value is _ABSENT:
                _guarded(lambda name=name: delattr(os.path, name), None)
            else:
                setattr(os.path, name, value)


_T = TypeVar("_T")

PROTOCOL = 3

# Every text field an event carries is cut to this many characters.
MAX_TEXT = 8000
# Captured output is kept per stream up to this many characters, from the end.
MAX_OUTPUT = 20000
# One-line fields (a failure's message, a frame's source and function, a
# reason, a sub-outcome's name) are cut to this many characters.
MAX_LINE = 1000
# A value a failure shows (a local, an entry of a comparison) is cut to this
# many characters.
MAX_VALUE = 200
# The innermost project frame shows at most this many locals, and this many
# lines before its own.
MAX_LOCALS = 20
BEFORE = 2
# A comparison lists at most this many differing entries, and descends at
# most this many levels into the one entry that differs.
MAX_DIFFERENCES = 50
MAX_DEPTH = 5


# ------------------------------------------------------------------ outcomes


class Outcome(str, enum.Enum):
    """The fixed outcome set. Adding a member is a breaking change."""

    PASSED = "passed"
    FAILED = "failed"
    ERROR = "error"
    SKIPPED = "skipped"
    EXPECTED_FAILURE = "expected_failure"
    UNEXPECTED_PASS = "unexpected_pass"

    @property
    def severity(self) -> int:
        return _SEVERITY[self]

    @property
    def is_bad(self) -> bool:
        return self in (Outcome.FAILED, Outcome.ERROR)


_SEVERITY = {
    Outcome.PASSED: 0,
    Outcome.SKIPPED: 1,
    Outcome.EXPECTED_FAILURE: 2,
    Outcome.UNEXPECTED_PASS: 3,
    Outcome.FAILED: 4,
    Outcome.ERROR: 5,
}


def worst(outcomes: Iterable[Outcome]) -> Outcome:
    """The most severe outcome, or ``PASSED`` for none."""
    return max(outcomes, key=lambda o: o.severity, default=Outcome.PASSED)


class Phase(str, enum.Enum):
    SETUP = "setup"
    TEST = "test"
    TEARDOWN = "teardown"


class Skip(Exception):
    """Raised to skip the current test or lifetime."""

    def __init__(self, reason: str = "") -> None:
        super().__init__(reason)
        self.reason = reason


# Exception types that mean "skipped", by qualified name, so that the engine
# needn't import the frameworks. Adapters may add their own.
SKIP_EXCEPTIONS = {
    "aye_engine.Skip",
    "unittest.case.SkipTest",
    "_pytest.outcomes.Skipped",
}

# The frameworks' skip exceptions, where they keep them: checked with
# ``isinstance`` when the framework is loaded.
_SKIP_CLASSES = (("unittest", "SkipTest"), ("_pytest.outcomes", "Skipped"))

# What pytest's ``OutcomeException`` sets on every instance.
_PYTEST_OUTCOME_ATTRIBUTES = ("msg", "pytrace")


def qualified_name(exc_type: type) -> str:
    module = getattr(exc_type, "__module__", "") or ""
    name = getattr(exc_type, "__qualname__", exc_type.__name__)
    return name if module == "builtins" else f"{module}.{name}"


def _is_pytest_skip(cls: type, exc: BaseException) -> bool:
    """pytest's ``Skipped`` (``pytest.skip``, ``importorskip``): it gives the
    class the module ``builtins``, so that it prints shorter, and no builtin
    is named ``Skipped``. Told by its name and module, then by its base
    ``OutcomeException`` or the attributes pytest sets."""
    if cls.__name__ != "Skipped" or getattr(cls, "__module__", None) not in (
        "builtins",
        "_pytest.outcomes",
    ):
        return False
    if any(base.__name__ == "OutcomeException" for base in cls.__mro__):
        return True
    return all(hasattr(exc, name) for name in _PYTEST_OUTCOME_ATTRIBUTES)


def is_skip(exc: BaseException) -> bool:
    """Whether ``exc`` means "skipped": a class in its MRO is one of
    ``SKIP_EXCEPTIONS`` or pytest's ``Skipped``, or it is an instance of
    unittest's or pytest's skip exception, when those are loaded."""
    for cls in type(exc).__mro__:
        if qualified_name(cls) in SKIP_EXCEPTIONS or _is_pytest_skip(cls, exc):
            return True
    for module, name in _SKIP_CLASSES:
        cls = getattr(sys.modules.get(module), name, None)
        if isinstance(cls, type) and isinstance(exc, cls):
            return True
    return False


def skip_reason(exc: BaseException) -> str:
    reason = getattr(exc, "reason", None)
    if reason is None:
        reason = getattr(exc, "msg", None)
    if reason is None:
        reason = str(exc)
    return str(reason)


def classify(exc: BaseException | None) -> Outcome:
    """The engine's default mapping from what a test body raised to an outcome:
    nothing → passed, a skip → skipped, an ``AssertionError`` → failed,
    anything else → error."""
    if exc is None:
        return Outcome.PASSED
    if is_skip(exc):
        return Outcome.SKIPPED
    if isinstance(exc, AssertionError):
        return Outcome.FAILED
    return Outcome.ERROR


# ------------------------------------------------------------------ failures


_CUT = "[… truncated]"


def bound(text: str, limit: int = MAX_TEXT) -> tuple[str, bool]:
    """Cut ``text`` to at most ``limit`` characters, keeping the start."""
    if len(text) <= limit:
        return text, False
    keep = limit - len(_CUT) - 1
    if keep <= 0:
        return text[:limit], True
    return text[:keep] + "\n" + _CUT, True


def cut_line(text: str, limit: int = MAX_LINE) -> str:
    """Cut one line of text to at most ``limit`` characters, ending in ``…``
    when cut, so it stays one line."""
    if len(text) <= limit:
        return text
    return text[: max(limit - 1, 0)] + "…"


def bound_tail(text: str, limit: int = MAX_OUTPUT) -> tuple[str, bool]:
    """Cut ``text`` to at most ``limit`` characters, keeping the end."""
    if len(text) <= limit:
        return text, False
    keep = limit - len(_CUT) - 1
    if keep <= 0:
        return text[-limit:], True
    return _CUT + "\n" + text[-keep:], True


def _guarded(compute: Callable[[], _T], default: _T) -> _T:
    """``compute()``, or ``default`` when it raises: reporting a test must
    not fail, whatever the test broke."""
    try:
        return compute()
    except Exception:  # noqa: BLE001 - see above
        return default


def _absolute(path: str) -> str:
    """``os.path.abspath``, with the functions bound at import."""
    if _fullpath is not None:
        try:
            return _normpath(_fullpath(path))
        except (OSError, ValueError):
            pass
    if not _isabs(path):
        path = _join(_getcwd(), path)
    return _normpath(path)


class Paths:
    """Turns absolute paths into project-relative ones, and tells user code
    from the runner's, the engine's and the framework's. It uses only the
    functions the engine bound at import, so a test patching ``os.path``
    changes nothing it says."""

    def __init__(self, root: str, internal: Sequence[str] = ()) -> None:
        self.root = _absolute(_fspath(root))
        self.internal = tuple(internal)
        self._key = _normcase(self.root)
        self._prefix = self._key if self._key.endswith(_SEP) else self._key + _SEP
        self._prefixes: tuple[str, ...] | None = None

    def relative(self, path: str) -> str:
        """``path`` relative to the root, with ``/`` separators; as given
        when it is outside the root (or another drive) or a ``<…>`` name."""
        try:
            text = _fspath(path)
            if text.startswith("<"):
                return text
            absolute = _absolute(text)
            key = _normcase(absolute)
            if key == self._key:
                return "."
            if not key.startswith(self._prefix):
                return text.replace(_SEP, "/")
            return absolute[len(self._prefix) :].replace(_SEP, "/")
        except Exception:  # noqa: BLE001 - not a path after all
            return _guarded(lambda: str(path).replace(_SEP, "/"), "<unknown>")

    def is_internal(self, path: str) -> bool:
        if path.startswith(("<aye:", "<frozen ")):
            return True
        return any(part in path for part in self.internal)

    def is_project(self, path: str) -> bool:
        """Whether ``path`` is the project's own code: a file under the
        root, outside installed packages (``site-packages``,
        ``dist-packages``) and Python's own directories (a virtual
        environment inside the root)."""
        text = _fspath(path)
        if text.startswith("<") or self.is_internal(text):
            return False
        key = _normcase(_absolute(text))
        if not key.startswith(self._prefix):
            return False
        parts = key[len(self._prefix) :].split(_SEP)
        if "site-packages" in parts or "dist-packages" in parts:
            return False
        return not any(key.startswith(prefix) for prefix in self._python_prefixes())

    def _python_prefixes(self) -> tuple[str, ...]:
        """Python's own directories inside the root (a virtual environment
        there), each ending in a separator; once."""
        if self._prefixes is None:
            found = []
            for prefix in {sys.prefix, sys.base_prefix, sys.exec_prefix, sys.base_exec_prefix}:
                key = _normcase(_absolute(prefix))
                key = key if key.endswith(_SEP) else key + _SEP
                if key.startswith(self._prefix) and key != self._prefix:
                    found.append(key)
            self._prefixes = tuple(found)
        return self._prefixes


_SURROGATES = re.compile("[\ud800-\udfff]")


def clean(text: str) -> str:
    """``text`` with every lone surrogate replaced by U+FFFD, so that it
    encodes as UTF-8. Surrogates come from bytes Python could not decode
    (``os.fsdecode`` of a file name, ``surrogateescape``); one character
    stands for one, so bounds still hold."""
    if text.isascii():
        return text
    return _SURROGATES.sub("\ufffd", text)


def _non_finite(value: Any) -> bool:
    return isinstance(value, float) and not isfinite(value)


def sanitize(value: Any) -> Any:
    """``value`` made safe for the protocol, recursively: strings (and keys)
    are ``clean``, and non-finite floats, which JSON cannot carry, are dropped
    from objects (an optional field is omitted) and become ``None`` in lists.
    Objects and lists (tuples too) are always new ones. Every event goes
    through it, so the values events are made of (text, numbers, ``None``,
    of exactly these types) are handled where they stand, without a call."""
    kind = type(value)
    if kind is str:
        return value if value.isascii() else _SURROGATES.sub("\ufffd", value)
    if kind is dict or (kind is not list and kind is not tuple and isinstance(value, dict)):
        return _sanitize_dict(value)
    if kind is list or kind is tuple or isinstance(value, (list, tuple)):
        return _sanitize_list(value)
    if isinstance(value, str):
        return clean(value)
    return None if _non_finite(value) else value


def _sanitize_dict(value: dict[Any, Any]) -> dict[Any, Any]:
    """``sanitize`` of an object: its keys ``clean``, its values sanitized,
    those that are non-finite floats left out."""
    found: dict[Any, Any] = {}
    for key, item in value.items():
        if type(key) is str:
            if not key.isascii():
                key = _SURROGATES.sub("\ufffd", key)  # noqa: PLW2901 - the clean key
        elif isinstance(key, str):
            key = clean(key)  # noqa: PLW2901 - the clean key
        kind = type(item)
        if kind is str:
            found[key] = item if item.isascii() else _SURROGATES.sub("\ufffd", item)
        elif kind is int or kind is bool or item is None:
            found[key] = item
        elif kind is float:
            if isfinite(item):
                found[key] = item
        elif not _non_finite(item):
            found[key] = sanitize(item)
    return found


def _sanitize_list(value: list[Any] | tuple[Any, ...]) -> list[Any]:
    """``sanitize`` of a list or tuple: a list of its items sanitized, those
    that are non-finite floats as ``None``."""
    found: list[Any] = []
    for item in value:
        kind = type(item)
        if kind is str:
            found.append(item if item.isascii() else _SURROGATES.sub("\ufffd", item))
        elif kind is int or kind is bool or item is None:
            found.append(item)
        elif kind is float:
            found.append(item if isfinite(item) else None)
        else:
            found.append(None if _non_finite(item) else sanitize(item))
    return found


_NUMBERS = re.compile(r"\b0x[0-9a-fA-F]+\b|\b\d+\b")


def signature(
    exception: str | None,
    message: str,
    location: tuple[str, int] | None,
    site: str | None = None,
) -> str:
    """Groups identical failures: the same exception, the same message once
    numbers and addresses are blanked out, raised at the same place. Without
    a location, ``site`` (what the adapter knows of the place: a doctest's
    test and example) stands for it, so that failures in different places
    that only read alike aren't grouped."""
    normalised = _NUMBERS.sub("N", clean(message))
    if location:
        where = clean(f"{location[0]}:{location[1]}")
    else:
        where = clean(f"\0{site}") if site else ""
    text = clean(f"{exception}\0{normalised}\0{where}")
    digest = sha1(text.encode("utf-8", "surrogatepass"), usedforsecurity=False)
    return digest.hexdigest()[:16]


class Failure:
    """A structured failure: what was raised, where, and the user's frames.
    ``site`` names the place for the signature when there is no location;
    it isn't sent. ``resource`` names the resource whose setup or teardown
    raised it; ``hint`` is one line suggesting a fix. ``assertion`` may
    carry, beside ``op``, ``left``, ``right`` and ``diff``, the keys
    ``expected`` (``"left"`` or ``"right"``), ``left_source``,
    ``right_source`` and ``compare`` (``compare``'s result)."""

    __slots__ = (
        "assertion",
        "detail",
        "exception",
        "frames",
        "hint",
        "location",
        "message",
        "phase",
        "resource",
        "site",
    )

    def __init__(  # noqa: PLR0913 - keyword-only fields
        self,
        message: str,
        *,
        phase: Phase | None = None,
        exception: str | None = None,
        assertion: dict[str, Any] | None = None,
        location: tuple[str, int] | None = None,
        frames: list[dict[str, Any]] | None = None,
        detail: str | None = None,
        site: str | None = None,
        resource: str | None = None,
        hint: str | None = None,
    ) -> None:
        self.message = message
        self.phase = phase
        self.exception = exception
        self.assertion = assertion
        self.location = location
        self.frames = frames or []
        self.detail = detail
        self.site = site
        self.resource = resource
        self.hint = hint

    @classmethod
    def from_exception(
        cls,
        exc: BaseException,
        paths: Paths,
        *,
        phase: Phase | None = None,
        assertion: dict[str, Any] | None = None,
        detail: str | None = None,
    ) -> Failure:
        """The failure ``exc`` describes. It never raises: what can't be
        read is left out (a frame, a broken ``__str__``)."""
        frames = _guarded(lambda: user_frames(exc.__traceback__, paths, exc), [])
        location = (frames[-1]["path"], frames[-1]["line"]) if frames else None
        if detail is None:
            detail = _guarded(lambda: format_exception(exc, paths), None)
        return cls(
            _guarded(lambda: exception_message(exc), type(exc).__name__),
            phase=phase,
            exception=type(exc).__name__,
            assertion=assertion,
            location=location,
            frames=frames,
            detail=detail,
        )

    def to_dict(self) -> dict[str, Any]:
        """The wire format, bounded as docs/events.md says, with every string
        ``clean``: the signature hashes the text actually sent."""
        lines = self.message.splitlines() if self.message else []
        message = cut_line(clean(lines[0] if lines else ""))
        data: dict[str, Any] = {
            "phase": self.phase.value if self.phase else None,
            "message": message,
        }
        exception = cut_line(clean(self.exception)) if self.exception else None
        if exception:
            data["exception"] = exception
        if self.assertion:
            data["assertion"] = _bounded_assertion(self.assertion)
        location = None
        if self.location:
            location = (clean(str(self.location[0])), self.location[1])
            data["location"] = {"path": location[0], "line": location[1]}
        if self.frames:
            data["frames"] = [_bounded_frame(frame) for frame in self.frames[-30:]]
        data["signature"] = signature(exception, message, location, self.site)
        if self.detail:
            data["detail"] = bound(self.detail)[0]
        if self.resource:
            data["resource"] = cut_line(str(self.resource))
        if self.hint:
            data["hint"] = cut_line(str(self.hint))
        return sanitize(data)


def _bounded_assertion(assertion: dict[str, Any]) -> dict[str, Any]:
    """An assertion's wire format: its operands and diff bounded, and the
    keys that describe it (``expected``, the operands' sources,
    ``compare``) when they are set."""
    data: dict[str, Any] = {
        "op": assertion.get("op", ""),
        "left": bound(str(assertion.get("left", "")), 2000)[0],
        "right": bound(str(assertion.get("right", "")), 2000)[0],
        "diff": [bound(str(line), 500)[0] for line in assertion.get("diff", [])[:200]],
    }
    if assertion.get("expected") in ("left", "right"):
        data["expected"] = assertion["expected"]
    for key in ("left_source", "right_source"):
        if isinstance(assertion.get(key), str):
            data[key] = bound(assertion[key], 2000)[0]
    if isinstance(assertion.get("compare"), dict):
        data["compare"] = assertion["compare"]
    return data


def _bounded_frame(frame: dict[str, Any]) -> dict[str, Any]:
    """A frame with its source line and function name cut to ``MAX_LINE``
    (its span kept inside the source), the lines before it too, and its
    locals bounded."""
    frame = dict(frame)
    for key in ("function", "source"):
        if isinstance(frame.get(key), str):
            frame[key] = cut_line(frame[key])
    cols = frame.get("cols")
    if cols is not None:
        source = frame.get("source") or ""
        if cols[0] < len(source):
            frame["cols"] = [cols[0], min(cols[1], len(source))]
        else:
            del frame["cols"]
    if "before" in frame:
        frame["before"] = [cut_line(str(line)) for line in frame["before"][-BEFORE:]]
    if "locals" in frame:
        frame["locals"] = [
            [cut_line(str(name), MAX_VALUE), cut_line(str(value), MAX_VALUE)]
            for name, value in frame["locals"][:MAX_LOCALS]
        ]
    return frame


def exception_message(exc: BaseException) -> str:
    text = str(exc).strip()
    if not text:
        return type(exc).__name__
    first = text.splitlines()[0]
    if isinstance(exc, AssertionError):
        return first
    return f"{type(exc).__name__}: {first}"


def format_exception(exc: BaseException, paths: Paths | None = None) -> str:
    """``exc`` and its traceback, as Python prints them. With ``paths``, the
    frames before the user's code (the worker's, the engine's, the
    framework's: ``Paths.is_internal``) are left out, as unittest leaves out
    its own."""
    tb = exc.__traceback__
    if paths is not None:
        while tb is not None and paths.is_internal(tb.tb_frame.f_code.co_filename):
            tb = tb.tb_next
    return "".join(_format_exception(type(exc), exc, tb))


def user_frames(tb: Any, paths: Paths, exc: BaseException | None = None) -> list[dict[str, Any]]:
    """The traceback's frames outside the runner, the engine and the
    framework, outermost first, described by ``describe_frames`` (``exc``,
    when given, is what was raised)."""
    frames = []
    entries = []
    walked = _walk(tb)
    summaries = extract_tb(tb)
    paired = len(walked) == len(summaries)
    for index, summary in enumerate(summaries):
        if paths.is_internal(summary.filename):
            continue
        frame: dict[str, Any] = {
            "path": paths.relative(summary.filename),
            "line": summary.lineno or 0,
            "function": summary.name,
        }
        source = (summary.line or getline(summary.filename, summary.lineno or 0)).strip()
        if source:
            frame["source"] = source
        frames.append(frame)
        if paired:
            entries.append(walked[index])
    if paired:
        describe_frames(frames, entries, paths, exc)
    return frames


def _walk(tb: Any) -> list[Any]:
    """The entries of a traceback, outermost first."""
    entries = []
    while tb is not None:
        entries.append(tb)
        tb = tb.tb_next
    return entries


def describe_frames(
    frames: list[dict[str, Any]],
    entries: Sequence[Any],
    paths: Paths,
    exc: BaseException | None = None,
) -> None:
    """Fill in what a failure's frames show beside their line, from their
    traceback entries (``entries``, one per frame): each frame's ``cols``,
    the span of the instruction that was running (Python 3.11+); and on the
    innermost frame of the project (``Paths.is_project``), the lines
    ``before`` it and its ``locals``. When ``exc`` is a rewritten assert's
    failure raised in that frame, ``cols`` is its actual operand's span when
    the expected side is known, else its test's (none when the test starts
    on a later line), on every Python. A span covering the whole source
    line is left out. Never raises: what can't be read is left out."""
    if len(frames) != len(entries):
        return
    innermost = None
    for index, (frame, tb) in enumerate(zip(frames, entries, strict=True)):
        cols = _guarded(lambda frame=frame, tb=tb: _instruction_span(frame, tb), None)
        if cols is not None:
            frame["cols"] = cols
        if _guarded(lambda tb=tb: paths.is_project(tb.tb_frame.f_code.co_filename), False):
            innermost = index
    if innermost is None:
        return
    frame, tb = frames[innermost], entries[innermost]
    if (
        tb.tb_next is None
        and exc is not None
        and _guarded(lambda: hasattr(exc, "_aye_span"), False)
    ):
        frame.pop("cols", None)
        cols = _guarded(lambda: _assert_span(frame, tb, exc), None)
        if cols is not None:
            frame["cols"] = cols
    before, indent = _guarded(lambda: _lines_before(frame, tb), ([], 0))
    if before:
        frame["before"] = before
        if indent:
            frame["indent"] = indent
    found = _guarded(lambda: local_values(tb.tb_frame.f_locals), [])
    if found:
        frame["locals"] = found


def _raw_line(tb: Any, line: int) -> str:
    """A line of the frame's file, without its end."""
    return getline(tb.tb_frame.f_code.co_filename, line).rstrip("\r\n")


def _instruction_span(frame: dict[str, Any], tb: Any) -> list[int] | None:
    """The span of the instruction the frame was running, on its line
    (``co_positions``: Python 3.11+)."""
    positions = getattr(tb.tb_frame.f_code, "co_positions", None)
    if positions is None or tb.tb_lasti < 0:
        return None
    found = next(islice(positions(), tb.tb_lasti // 2, None), None)
    if found is None:
        return None
    first, last, start, end = found
    line = frame.get("line")
    if first != line or start is None:
        return None
    raw = _raw_line(tb, line)
    encoded = raw.encode("utf-8", "surrogatepass")
    start = len(encoded[:start].decode("utf-8", "replace"))
    end = (
        len(encoded[:end].decode("utf-8", "replace")) if last == line and end is not None else None
    )
    return _source_span(raw, start, end)


def _assert_span(frame: dict[str, Any], tb: Any, exc: BaseException) -> list[int] | None:
    """Where a rewritten assert's failure points: its actual operand when
    the expected side is known, else its test (spans in characters on the
    assert's first line, from ``aye_rewrite``)."""
    span = exc._aye_span  # type: ignore[attr-defined]
    data = getattr(exc, "_aye_assert", None)
    if isinstance(data, dict):
        actual = {"right": "left_cols", "left": "right_cols"}.get(data.get("expected"))
        if actual is not None and data.get(actual) is not None:
            span = data[actual]
    if span is None:
        return None
    start, end = span
    return _source_span(_raw_line(tb, frame.get("line", 0)), int(start), int(end))


def _source_span(raw: str, start: int, end: int | None) -> list[int] | None:
    """A span on the line ``raw`` (characters; ``end`` ``None``: to the
    line's end) as ``cols``, in the line without its indentation: ``None``
    when it is empty or the whole of it."""
    source = raw.strip()
    indent = len(raw) - len(raw.lstrip())
    start = max(start - indent, 0)
    end = len(source) if end is None else min(end - indent, len(source))
    if start >= end or (start == 0 and end == len(source)):
        return None
    return [start, end]


def _lines_before(frame: dict[str, Any], tb: Any) -> tuple[list[str], int]:
    """The lines just before the frame's, at most ``BEFORE``, oldest first,
    and how far the frame's own line is indented beyond them: the lines
    keep their indentation relative to the least indented of them and the
    frame's line (a blank line is ``""``)."""
    line = frame.get("line") or 0
    raw = _raw_line(tb, line)
    if not raw.strip():
        return [], 0
    texts = [_raw_line(tb, number).rstrip() for number in range(max(line - BEFORE, 1), line)]

    def lead(text: str) -> int:
        return len(text) - len(text.lstrip())

    indent = lead(raw)
    common = min([indent, *(lead(text) for text in texts if text)])
    return [text[common:] for text in texts], indent - common


def local_values(namespace: Any) -> list[list[str]]:
    """A frame's locals as a failure shows them: sorted by name, without
    dunders, modules, functions, classes, the assertion rewriter's
    temporaries or a ``self`` that shows nothing but its address; at most
    ``MAX_LOCALS``, each value by its safe repr, cut to ``MAX_VALUE``
    characters."""
    found: list[list[str]] = []
    for name in sorted(name for name in namespace if isinstance(name, str)):
        if len(found) >= MAX_LOCALS:
            break
        if name.startswith("@") or (name.startswith("__") and name.endswith("__")):
            continue
        try:
            value = namespace[name]
        except Exception:  # noqa: BLE001, S112 - a cell not bound yet
            continue
        if isinstance(value, _NOT_LOCALS):
            continue
        if name == "self" and type(value).__repr__ is object.__repr__:
            continue
        found.append([name, saferepr(value, MAX_VALUE)])
    return found


_NOT_LOCALS = (
    ModuleType,
    type,
    FunctionType,
    BuiltinFunctionType,
    MethodType,
    BuiltinMethodType,
)


# ------------------------------------------------------------------ values


class _SafeRepr(reprlib.Repr):
    """``repr`` bounded as pytest's ``saferepr``: containers cut by
    ``reprlib``'s limits (a dict in its own order), the whole cut to
    ``maxsize`` characters in the middle, and a broken ``__repr__``
    described rather than raised (raised when ``strict``)."""

    def __init__(self, maxsize: int, strict: bool = False) -> None:
        super().__init__()
        self.maxstring = self.maxother = maxsize
        self.maxsize = maxsize
        self.strict = strict

    def repr(self, x: Any) -> str:
        try:
            text = super().repr(x)
        except (KeyboardInterrupt, SystemExit):
            raise
        except BaseException as exc:  # shown, unless strict
            if self.strict:
                raise
            text = _repr_failed(exc, x)
        return _ellipsize(text, self.maxsize)

    def repr_instance(self, x: Any, level: int) -> str:
        try:
            text = repr(x)
        except (KeyboardInterrupt, SystemExit):
            raise
        except BaseException as exc:  # shown, unless strict
            if self.strict:
                raise
            text = _repr_failed(exc, x)
        return _ellipsize(text, self.maxsize)

    def repr_dict(self, x: dict[Any, Any], level: int) -> str:
        if not x:
            return "{}"
        if level <= 0:
            return "{...}"
        pieces = [
            f"{self.repr1(key, level - 1)}: {self.repr1(x[key], level - 1)}"
            for key in islice(x, self.maxdict)
        ]
        if len(x) > self.maxdict:
            pieces.append("...")
        return "{" + ", ".join(pieces) + "}"


def _repr_failed(exc: BaseException, value: Any) -> str:
    try:
        raised = repr(exc)
    except Exception:  # noqa: BLE001 - its type, then
        raised = type(exc).__name__
    return f"<[{raised} raised in repr()] {type(value).__name__} object at 0x{id(value):x}>"


def _ellipsize(text: str, maxsize: int) -> str:
    if len(text) <= maxsize:
        return text
    head = max(0, (maxsize - 3) // 2)
    tail = max(0, maxsize - 3 - head)
    return text[:head] + "..." + text[len(text) - tail :]


def saferepr(value: Any, maxsize: int = MAX_VALUE) -> str:
    """``repr(value)`` as a failure shows it: bounded (``_SafeRepr``) and
    never raising (but for an interrupt)."""
    return _SafeRepr(maxsize).repr(value)


def _strict_repr(value: Any) -> str:
    """A compared entry's repr, bounded as ``saferepr``'s, raising what its
    ``__repr__`` raises: ``compare`` then says nothing."""
    return _SafeRepr(MAX_VALUE, strict=True).repr(value)


# ------------------------------------------------------------------ comparisons

# What one side has where the other has nothing.
_ABSENT_ENTRY = object()

# The kinds of values a comparison descends into.
_CONTAINERS = frozenset({"dict", "list", "tuple", "set", "dataclass", "namedtuple"})


def compare(left: Any, right: Any, expected: str | None = None) -> dict[str, Any] | None:
    """How ``left`` and ``right``, which ``==`` found unequal, differ, by
    their structure: the wire format of an assertion's ``compare``
    (``{kind, summary, same?, differs?, omitted?}``), or ``None`` when they
    aren't values of one kind it knows (dicts and other mappings, lists,
    tuples, sets, strings, bytes, dataclasses, namedtuples, dates and
    times, numbers), or when comparing or showing them raised.
    ``expected`` names the operand holding the expected value (``"left"``
    or ``"right"``): what only it has is ``missing``, what only the other
    has ``extra``; without it, ``only in left`` and ``only in right``.

    When exactly one entry differs and both of its values are containers of
    one kind, it describes them instead, at most ``MAX_DEPTH`` levels down:
    the entries' ``at`` then start with the way there (``['a'][0].b``), and
    so does the summary. At most ``MAX_DIFFERENCES`` entries are listed,
    the rest counted in ``omitted``; each value is its safe repr, cut to
    ``MAX_VALUE`` characters."""
    try:
        return _compare(left, right, expected, "", MAX_DEPTH)
    except (KeyboardInterrupt, SystemExit):
        raise
    except BaseException:  # noqa: BLE001 - no comparison, then
        return None


class _Found:
    """What one level of a comparison found: the entries that differ, as
    ``(at, left, right)`` (``_ABSENT_ENTRY`` for a side without one)."""

    __slots__ = ("differs", "kind", "same", "summary")

    def __init__(
        self, kind: str, summary: str, same: int, differs: list[tuple[str | None, Any, Any]]
    ) -> None:
        self.kind = kind
        self.summary = summary
        self.same = same
        self.differs = differs


def _compare(
    left: Any, right: Any, expected: str | None, at: str, depth: int
) -> dict[str, Any] | None:
    kind = _kind_of(left, right)
    if kind is None:
        return None
    found = _COMPARERS[kind](left, right, expected)
    if found is None:
        return None
    if depth > 1 and len(found.differs) == 1 and kind in _CONTAINERS:
        where, inner_left, inner_right = found.differs[0]
        if (
            where is not None
            and inner_left is not _ABSENT_ENTRY
            and inner_right is not _ABSENT_ENTRY
            and _kind_of(inner_left, inner_right) in _CONTAINERS
        ):
            inner = _compare(inner_left, inner_right, expected, at + where, depth - 1)
            if inner is not None:
                return inner
    data: dict[str, Any] = {
        "kind": found.kind,
        "summary": f"{at}: {found.summary}" if at else found.summary,
    }
    if found.same:
        data["same"] = found.same
    differs = []
    for where, one, other in found.differs[:MAX_DIFFERENCES]:
        entry: dict[str, Any] = {}
        if where is not None:
            entry["at"] = at + where
        if one is not _ABSENT_ENTRY:
            entry["left"] = _entry(one)
        if other is not _ABSENT_ENTRY:
            entry["right"] = _entry(other)
        differs.append(entry)
    if differs:
        data["differs"] = differs
    if len(found.differs) > MAX_DIFFERENCES:
        data["omitted"] = len(found.differs) - MAX_DIFFERENCES
    return data


def _entry(value: Any) -> str:
    """An entry as a comparison shows it: its safe repr, or the text a
    comparer made of it already (``_Shown``)."""
    if isinstance(value, _Shown):
        return _ellipsize(str(value), MAX_VALUE)
    return _strict_repr(value)


def _kind_of(left: Any, right: Any) -> str | None:
    """The kind both values are, as ``compare`` knows it; ``None`` when they
    are of different kinds or of none it knows. A namedtuple and another
    tuple (or another type's namedtuple) compare as tuples, as ``==`` does;
    two dataclasses must be of one type."""
    kinds = {_kind(left), _kind(right)}
    if None in kinds:
        return None
    if kinds == {"namedtuple", "tuple"}:
        return "tuple"
    if len(kinds) != 1:
        return None
    (kind,) = kinds
    if kind in ("dataclass", "namedtuple") and type(left) is not type(right):
        return "tuple" if kind == "namedtuple" else None
    return kind


def _kind(value: Any) -> str | None:  # noqa: PLR0911, PLR0912 - one return per kind
    if isinstance(value, bool):
        return None
    if isinstance(value, str):
        return "str"
    if isinstance(value, (bytes, bytearray)):
        return "bytes"
    datetime = sys.modules.get("datetime")
    if datetime is not None:
        if isinstance(value, datetime.datetime):
            return "datetime"
        if isinstance(value, datetime.date):
            return "date"
        if isinstance(value, datetime.time):
            return "time"
    if hasattr(type(value), "__dataclass_fields__") and not isinstance(value, type):
        return "dataclass"
    if isinstance(value, tuple) and hasattr(type(value), "_fields"):
        return "namedtuple"
    if isinstance(value, Mapping):
        return "dict"
    if isinstance(value, AbstractSet):
        return "set"
    if isinstance(value, list):
        return "list"
    if isinstance(value, tuple):
        return "tuple"
    import numbers  # noqa: PLC0415 - only when a test failed

    if isinstance(value, numbers.Number):
        return "number"
    return None


def _equal(left: Any, right: Any) -> bool:
    """Whether two entries are equal, as a container compares them."""
    return left is right or bool(left == right)


def _plural(count: int, noun: str) -> str:
    return noun if count == 1 else noun + "s"


def _one_sided(left_only: int, right_only: int, expected: str | None, noun: str = "") -> str:
    """``1 key missing, 2 extra``: the entries only one side has, missing
    from the actual value or extra in it when the expected side is known,
    else ``only in left`` or ``only in right``."""
    if expected == "right":
        parts = [(right_only, "missing"), (left_only, "extra")]
    elif expected == "left":
        parts = [(left_only, "missing"), (right_only, "extra")]
    else:
        parts = [(left_only, "only in left"), (right_only, "only in right")]
    texts = []
    for count, word in parts:
        if count:
            named = f" {_plural(count, noun)}" if noun and not texts else ""
            texts.append(f"{count}{named} {word}")
    return ", ".join(texts)


def _summary(kind: str, differing: int, common: int, among: str, sided: str) -> str | None:
    """``dict differs in 1 of 2 keys; 1 key missing``, ``list: 1 item
    extra``: how many of the entries both sides have differ (``among``:
    ``in … keys``, ``at … positions``), then what only one side has."""
    if differing:
        preposition, noun = among.split()
        text = f"{kind} differs {preposition} {differing} of {common} {_plural(common, noun)}"
        return f"{text}; {sided}" if sided else text
    return f"{kind}: {sided}" if sided else None


def _compare_dict(left: Any, right: Any, expected: str | None) -> _Found | None:
    """The keys whose values differ, then those only one side has."""
    same = 0
    differs: list[tuple[str | None, Any, Any]] = []
    left_only: list[tuple[str | None, Any, Any]] = []
    right_only: list[tuple[str | None, Any, Any]] = []
    for key in left:
        if key in right:
            if _equal(left[key], right[key]):
                same += 1
            else:
                differs.append((f"[{_strict_repr(key)}]", left[key], right[key]))
        else:
            left_only.append((f"[{_strict_repr(key)}]", left[key], _ABSENT_ENTRY))
    for key in right:
        if key not in left:
            right_only.append((f"[{_strict_repr(key)}]", _ABSENT_ENTRY, right[key]))
    sided = _one_sided(len(left_only), len(right_only), expected, "key")
    summary = _summary("dict", len(differs), same + len(differs), "in key", sided)
    if summary is None:
        return None
    return _Found("dict", summary, same, [*differs, *left_only, *right_only])


def _compare_sequence(left: Any, right: Any, expected: str | None) -> _Found | None:
    """The positions both sides have whose items differ, then the items
    only the longer one has."""
    kind = "list" if isinstance(left, list) else "tuple"
    common = min(len(left), len(right))
    same = 0
    differs: list[tuple[str | None, Any, Any]] = []
    for index in range(common):
        if _equal(left[index], right[index]):
            same += 1
        else:
            differs.append((f"[{index}]", left[index], right[index]))
    left_only = [(f"[{i}]", left[i], _ABSENT_ENTRY) for i in range(common, len(left))]
    right_only = [(f"[{i}]", _ABSENT_ENTRY, right[i]) for i in range(common, len(right))]
    sided = _one_sided(len(left_only), len(right_only), expected, "item")
    if differs and not sided and _reordered(left, right):
        sided = "the same items in another order"
    summary = _summary(kind, len(differs), common, "at position", sided)
    if summary is None:
        return None
    return _Found(kind, summary, same, [*differs, *left_only, *right_only])


def _reordered(left: Sequence[Any], right: Sequence[Any]) -> bool:
    """Whether two sequences of one length hold the same items (hashable
    ones: anything else is never said to be reordered)."""
    try:
        from collections import Counter  # noqa: PLC0415 - only when a test failed

        return Counter(left) == Counter(right)
    except Exception:  # noqa: BLE001 - unhashable items, a broken __eq__
        return False


def _compare_set(left: Any, right: Any, expected: str | None) -> _Found | None:
    """The items only one side has, in the order of their reprs."""
    left_only = sorted(_strict_repr(item) for item in left if item not in right)
    right_only = sorted(_strict_repr(item) for item in right if item not in left)
    sided = _one_sided(len(left_only), len(right_only), expected)
    if not sided:
        return None
    same = sum(1 for item in left if item in right)
    differs: list[tuple[str | None, Any, Any]] = [
        *((None, _Shown(text), _ABSENT_ENTRY) for text in left_only),
        *((None, _ABSENT_ENTRY, _Shown(text)) for text in right_only),
    ]
    return _Found("set", f"set: {sided}", same, differs)


class _Shown(str):
    """An entry a comparer has shown as text already (a set's item, a line
    of text, a date in ISO format)."""

    __slots__ = ()


def _compare_fields(left: Any, right: Any, expected: str | None) -> _Found | None:
    kind = _kind(left)
    if kind == "dataclass":
        import dataclasses  # noqa: PLC0415 - only when a test failed

        names = [field.name for field in dataclasses.fields(left) if field.compare]
    else:
        names = list(type(left)._fields)
    same = 0
    differs: list[tuple[str | None, Any, Any]] = []
    for name in names:
        one, other = getattr(left, name), getattr(right, name)
        if _equal(one, other):
            same += 1
        else:
            differs.append((f".{name}", one, other))
    if not differs:
        return None
    count = len(names)
    summary = (
        f"{type(left).__name__} differs in {len(differs)} of {count} {_plural(count, 'field')}"
    )
    return _Found(kind or "dataclass", summary, same, differs)


def _compare_text(left: str, right: str, expected: str | None) -> _Found | None:
    """Where two strings first differ, ``line L, column C`` (both from 1),
    and the line each has there, by its repr: without its line end, unless
    the ends are what differs; absent on a side with no such line."""
    lefts, rights = left.splitlines(keepends=True), right.splitlines(keepends=True)
    line = 0
    while line < len(lefts) and line < len(rights) and lefts[line] == rights[line]:
        line += 1
    one = lefts[line] if line < len(lefts) else None
    other = rights[line] if line < len(rights) else None
    column = _common_prefix(one or "", other or "")
    bare = [text.rstrip("\r\n") if text is not None else None for text in (one, other)]
    if bare[0] != bare[1]:
        one, other = bare
    where = f"line {line + 1}, column {column + 1}"
    shown = [
        _text_line(text, column) if text is not None else _ABSENT_ENTRY for text in (one, other)
    ]
    return _Found("str", f"strings differ at {where}", 0, [(where, shown[0], shown[1])])


def _common_prefix(one: Sequence[Any], other: Sequence[Any]) -> int:
    """How many items two sequences start with in common."""
    index = 0
    while index < len(one) and index < len(other) and one[index] == other[index]:
        index += 1
    return index


def _text_line(text: str, column: int) -> _Shown:
    """A line of text as a comparison shows it: its repr, a piece of it
    around ``column`` when it is too long to show whole."""
    shown = repr(text)
    if len(shown) > MAX_VALUE:
        start = max(0, column - MAX_VALUE // 4)
        end = start + MAX_VALUE // 2
        shown = repr(text[start:end])
        shown = ("..." if start else "") + shown + ("..." if end < len(text) else "")
    return _Shown(shown)


def _compare_bytes(left: bytes, right: bytes, expected: str | None) -> _Found | None:
    """Where two byte strings first differ, ``[i]``, and each one there by
    its repr (a piece of it around ``i`` when it is too long)."""
    index = _common_prefix(left, right)
    summary = f"bytes differ at index {index}"
    if len(left) != len(right):
        summary += f"; lengths {len(left)} and {len(right)}"
    shown = [_bytes_window(bytes(value), index) for value in (left, right)]
    return _Found("bytes", summary, 0, [(f"[{index}]", shown[0], shown[1])])


def _bytes_window(value: bytes, index: int) -> _Shown:
    shown = repr(value)
    if len(shown) > MAX_VALUE:
        start = max(0, index - MAX_VALUE // 8)
        end = start + MAX_VALUE // 4
        shown = repr(value[start:end])
        shown = ("..." if start else "") + shown + ("..." if end < len(value) else "")
    return _Shown(shown)


def _compare_time(left: Any, right: Any, expected: str | None) -> _Found | None:
    """Dates and times, in ISO format, and how far apart they are (but
    for times, or an aware and a naive datetime)."""
    kind = _kind(left) or "datetime"
    summary = f"{kind} differs"
    try:
        summary += f" by {_duration(abs(left - right))}"
    except TypeError:
        pass
    shown = (_Shown(left.isoformat()), _Shown(right.isoformat()))
    return _Found(kind, summary, 0, [(None, *shown)])


def _duration(delta: Any) -> str:
    """``4 days, 9:10:00``, ``3 days``, ``0:00:01.500000``."""
    return str(delta).removesuffix(", 0:00:00")


def _compare_number(left: Any, right: Any, expected: str | None) -> _Found | None:
    """How far apart two numbers are, and nothing else: their values are
    the assertion's own."""
    difference = abs(left - right)
    if isinstance(difference, float):
        if not isfinite(difference):
            return None
        shown = f"{difference:.6g}"
    else:
        shown = str(difference)
    return _Found("number", f"differ by {shown}", 0, [])


_COMPARERS: dict[str, Callable[[Any, Any, str | None], _Found | None]] = {
    "dict": _compare_dict,
    "list": _compare_sequence,
    "tuple": _compare_sequence,
    "set": _compare_set,
    "dataclass": _compare_fields,
    "namedtuple": _compare_fields,
    "str": _compare_text,
    "bytes": _compare_bytes,
    "datetime": _compare_time,
    "date": _compare_time,
    "time": _compare_time,
    "number": _compare_number,
}


# ------------------------------------------------------------------ results


class Result:
    """What happened to one test. Folded into a ``test_finished`` event."""

    def __init__(self, id: str) -> None:
        self.id = id
        self.outcome: Outcome | None = None
        self.failures: list[Failure] = []
        self.cause: str | None = None
        self.reason: str | None = None
        self.stdout = ""
        self.stderr = ""
        self.log = ""
        self.duration = 0.0
        self.reproduce: dict[str, Any] | None = None
        # Name and value pairs the test recorded about itself, in order
        # (pytest's ``record_property``).
        self.properties: list[tuple[str, str]] = []
        # How long the body alone took, when the adapter times it apart from
        # setup and teardown (pytest's ``call`` phase).
        self.body_duration: float | None = None
        # The resources set up for a test with a bad outcome, in setup
        # order: (name, scope) pairs, the scope ``session``, ``package``,
        # ``module``, ``class`` or ``function``.
        self.resources: list[tuple[str, str]] = []

    def add(self, outcome: Outcome, failure: Failure | None = None) -> None:
        """Record an outcome from one phase; the worst one wins."""
        if self.outcome is None or outcome.severity > self.outcome.severity:
            self.outcome = outcome
        if failure is not None:
            self.failures.append(failure)

    def to_fields(self) -> dict[str, Any]:
        fields: dict[str, Any] = {
            "id": self.id,
            "outcome": (self.outcome or Outcome.PASSED).value,
            "duration_ms": round(self.duration * 1000, 3),
        }
        if self.failures:
            fields["failures"] = [f.to_dict() for f in self.failures]
        if self.cause:
            fields["cause"] = self.cause
        if self.reason:
            fields["reason"] = bound(self.reason, MAX_LINE)[0]
        output: dict[str, Any] = {}
        truncated = False
        for name in ("stdout", "stderr", "log"):
            text = getattr(self, name)
            if text:
                output[name], cut = bound_tail(text)
                truncated |= cut
        if truncated:
            output["truncated"] = True
        if output:
            fields["output"] = output
        if self.reproduce:
            fields["reproduce"] = self.reproduce
        if self.properties:
            fields["properties"] = [
                [cut_line(name), bound(value)[0]] for name, value in self.properties
            ]
        if self.body_duration is not None:
            fields["body_duration_ms"] = round(self.body_duration * 1000, 3)
        if self.resources:
            fields["resources"] = [
                {"name": cut_line(name), "scope": scope} for name, scope in self.resources
            ]
        return fields


def reproduce(
    test_id: str, framework_args: Sequence[str] = (), rerun_args: Sequence[str] = ()
) -> dict[str, Any]:
    """A test's ``reproduce`` field: ``aye``, aye's own options that shaped
    the run (``rerun_args``: ``-c FILE``), the id, then ``--`` and the
    arguments the user gave after it, when there are any, so that the rerun
    sees the same configuration and framework options (an option a
    ``pytest_generate_tests`` reads may decide which tests exist)."""
    command = ["aye", *(str(arg) for arg in rerun_args), test_id]
    if framework_args:
        command += ["--", *(str(arg) for arg in framework_args)]
    return {"command": command}


def rerun_args(context: Any) -> list[str]:
    """aye's own options a rerun repeats before the id: the context's
    ``rerun_args``, which the binary adds to every adapter's context of a
    batch when the run has some (``-c FILE``, as given)."""
    value = context.get("rerun_args") if isinstance(context, dict) else None
    if not isinstance(value, (list, tuple)):
        return []
    return [arg for arg in value if isinstance(arg, str)]


# ------------------------------------------------------------------ the sink


class Sink:
    """Where events go. ``write`` receives one JSON-ready dict per event; the
    worker frames it onto the protocol stream. Workers stamp the batch and the
    time; the runner stamps the rest of the envelope."""

    def __init__(self, write: Callable[[dict[str, Any]], None], batch: int | None = None) -> None:
        self._write = write
        self.batch = batch
        self._t0 = perf_counter()

    def emit(self, event: str, **fields: Any) -> None:
        """Write one record. It is ``sanitize``d first, so that no record can
        fail to serialize, whatever text a test produced."""
        record: dict[str, Any] = {}
        if self.batch is not None:
            record["batch"] = self.batch
        record["t_ms"] = round((perf_counter() - self._t0) * 1000, 3)
        record["event"] = event
        for key, value in fields.items():
            if value is not None:
                record[key] = value
        self._write(_sanitize_dict(record))

    def test_started(self, id: str) -> None:
        self.emit("test_started", id=id)

    def test_finished(self, result: Result) -> None:
        self.emit("test_finished", **result.to_fields())

    def sub_outcome(
        self,
        id: str,
        name: str,
        outcome: Outcome,
        failure: Failure | None = None,
        duration: float = 0.0,
        reason: str | None = None,
    ) -> None:
        """A result inside a test. ``reason`` says why a skipped or expected
        one was (unittest's ``skipTest`` in a ``subTest``)."""
        self.emit(
            "sub_outcome",
            id=id,
            name=cut_line(name),
            outcome=outcome.value,
            failure=failure.to_dict() if failure else None,
            reason=bound(reason, MAX_LINE)[0] if reason else None,
            duration_ms=round(duration * 1000, 3),
        )

    def lifetime_started(self, lifetime: str, kind: str) -> None:
        self.emit("lifetime_started", lifetime=lifetime, kind=kind)

    def lifetime_finished(self, lifetime: str, failure: Failure | None, duration: float) -> None:
        self.emit(
            "lifetime_finished",
            lifetime=lifetime,
            failure=failure.to_dict() if failure else None,
            duration_ms=round(duration * 1000, 3),
        )

    def diagnostic(self, severity: str, code: str, path: str, message: str) -> None:
        message = bound(message)[0]
        self.emit("diagnostic", severity=severity, code=code, path=path, message=message)

    def run_property(self, name: str, value: str) -> None:
        """A name and value pair about the whole run that a test recorded
        (pytest's ``record_testsuite_property``)."""
        self.emit("run_property", name=cut_line(name), value=bound(value)[0])


class ListSink(Sink):
    """A sink that keeps its events, for tests and for refinement."""

    def __init__(self, batch: int | None = None) -> None:
        self.events: list[dict[str, Any]] = []
        super().__init__(self.events.append, batch)


# ------------------------------------------------------------------ capture


class _Kept(BytesIO):
    """The bytes of a captured stream, still readable once a test closed it."""

    def __init__(self) -> None:
        super().__init__()
        self.kept = b""

    def close(self) -> None:
        if not self.closed:
            self.kept = self.getvalue()
        super().close()

    def value(self) -> bytes:
        return self.kept if self.closed else self.getvalue()


class CaptureIO(TextIOWrapper):
    """A captured ``sys.stdout`` or ``sys.stderr``, as pytest's default
    capture (``--capture=fd``) makes them: text encoded as UTF-8 into bytes,
    so that ``.buffer`` and ``.encoding`` work, with ``errors="replace"``,
    so that text UTF-8 can't hold (a lone surrogate) is written as ``?``
    rather than failing the test, and ``fileno()`` a real file (made when
    first asked for), whose writes are captured too (``os.write``, a
    subprocess given the stream). What was written stays readable after a
    test closes it."""

    def __init__(self) -> None:
        super().__init__(
            _Kept(), encoding="utf-8", errors="replace", newline="", write_through=True
        )
        self._file: Any = None

    def fileno(self) -> int:
        if self.closed:
            raise ValueError("I/O operation on closed file")
        if self._file is None:
            from tempfile import TemporaryFile  # noqa: PLC0415 - only fileno() needs it

            # It lives as long as the stream: ``release`` closes it.
            self._file = TemporaryFile()  # noqa: SIM115 - see above
        return int(self._file.fileno())

    def getvalue(self) -> str:
        """Everything written, through the stream or its file. Never raises."""
        data = _guarded(self._read_buffer, b"")
        if self._file is not None:
            data += _guarded(self._read_file, b"")
        return data.decode("utf-8", "replace")

    def _read_buffer(self) -> bytes:
        return bytes(self.buffer.value())

    def release(self) -> str:
        """``getvalue()``, then the file behind ``fileno()`` closed."""
        value = self.getvalue()
        if self._file is not None:
            _guarded(self._file.close, None)
        return value

    def _read_file(self) -> bytes:
        self._file.seek(0)
        return bytes(self._file.read())


@contextmanager
def captured(result: Result) -> Iterator[None]:
    """``sys.stdout`` and ``sys.stderr`` into ``result`` while the block
    runs (``CaptureIO``), restored afterwards whatever the block did."""
    out, err = CaptureIO(), CaptureIO()
    saved = sys.stdout, sys.stderr
    sys.stdout, sys.stderr = out, err
    try:
        yield
    finally:
        sys.stdout, sys.stderr = saved
        result.stdout += out.release()
        result.stderr += err.release()


# ------------------------------------------------------------------ services


class ServiceUnavailable(LookupError):
    pass


class Services:
    """What the runner provides inside the worker, looked up by name.

    Services are additive: an engine asks for a name and copes with ``None``.
    The names match ``aye_protocol::services`` on the Rust side.
    """

    CAPTURE = "capture"
    CONTEXT = "context"
    CLEANUP = "cleanup"
    COVERAGE = "coverage"
    SANDBOX = "sandbox"
    TIMEOUT = "timeout"
    RESOURCES = "resources"
    PATCHING = "patching"
    CLOCK = "clock"
    RANDOMNESS = "randomness"
    NETWORK = "network"
    FILESYSTEM = "filesystem"
    SNAPSHOTS = "snapshots"
    DATA = "data"

    def __init__(self, provided: dict[str, Any] | None = None) -> None:
        self._provided = dict(provided or {})

    # A ``capture`` service has ``capture(result)``: a context manager that
    # fills ``result.stdout``, ``result.stderr`` and ``result.log``.

    def get(self, name: str, default: Any = None) -> Any:
        return self._provided.get(name, default)

    def has(self, name: str) -> bool:
        return name in self._provided

    def require(self, name: str) -> Any:
        try:
            return self._provided[name]
        except KeyError:
            raise ServiceUnavailable(f"the runner did not provide the {name!r} service") from None

    def names(self) -> list[str]:
        return sorted(self._provided)


class Context:
    """The ``context`` service: which test and lifetimes are running now."""

    def __init__(self) -> None:
        self.test: str | None = None
        self.lifetimes: list[str] = []


class Cleanup:
    """The ``cleanup`` service: register teardown on the test running now.
    Runs last in, first out, even when setup failed halfway."""

    def __init__(self) -> None:
        self._stack: list[list[tuple[Callable[..., Any], tuple, dict]]] = []

    def push(self) -> None:
        self._stack.append([])

    def add(self, fn: Callable[..., Any], *args: Any, **kwargs: Any) -> None:
        if not self._stack:
            raise RuntimeError("no test or lifetime is running to attach a cleanup to")
        self._stack[-1].append((fn, args, kwargs))

    def pop(self) -> BaseException | None:
        """Run the innermost scope's cleanups; return the first exception."""
        callbacks = self._stack.pop() if self._stack else []
        first: BaseException | None = None
        for fn, args, kwargs in reversed(callbacks):
            try:
                fn(*args, **kwargs)
            except BaseException as exc:  # noqa: BLE001 - every cleanup must run
                if first is None:
                    first = exc
        return first


def default_services() -> Services:
    """The services the engine provides itself when the runner asks for them
    by name: context and cleanup."""
    return Services({Services.CONTEXT: Context(), Services.CLEANUP: Cleanup()})


# ------------------------------------------------------------------ lifetimes


class Lifetime:
    """Shared setup that holds while consecutive tests need it: a module's or
    a class's setup. Torn down in reverse order of setup; its cleanups run
    last in, first out, and run even when setup failed halfway.

    ``id`` is unique in the run and is the affinity key tests declare.
    """

    def __init__(
        self,
        id: str,
        kind: str,
        setup: Callable[[], Any] | None = None,
        teardown: Callable[[], Any] | None = None,
    ) -> None:
        self.id = id
        self.kind = kind
        self._setup = setup
        self._teardown = teardown
        self._cleanups: list[tuple[Callable[..., Any], tuple, dict]] = []
        self.failure: Failure | None = None
        self.skip_reason: str | None = None
        self.is_open = False
        self._started = 0.0

    def add_cleanup(self, fn: Callable[..., Any], *args: Any, **kwargs: Any) -> None:
        self._cleanups.append((fn, args, kwargs))

    @property
    def failed(self) -> bool:
        return self.failure is not None

    def open(self, sink: Sink, paths: Paths) -> bool:
        """Run setup. Returns whether tests inside may run."""
        self.is_open = True
        self._started = perf_counter()
        sink.lifetime_started(self.id, self.kind)
        if self._setup is None:
            return True
        try:
            self._setup()
        except BaseException as exc:
            if is_skip(exc):
                self.skip_reason = skip_reason(exc)
                return False
            self.failure = Failure.from_exception(exc, paths, phase=Phase.SETUP)
            if isinstance(exc, KeyboardInterrupt):
                # Recorded first, so that closing it reports the failure and
                # skips the teardown of a setup that never finished.
                raise
            return False
        return True

    def close(self, sink: Sink, paths: Paths) -> None:
        """Run cleanups (last in, first out), then teardown if setup ran."""
        if not self.is_open:
            return
        self.is_open = False
        errors: list[BaseException] = []
        for fn, args, kwargs in reversed(self._cleanups):
            try:
                fn(*args, **kwargs)
            except BaseException as exc:  # noqa: BLE001 - every cleanup must run
                errors.append(exc)
        self._cleanups.clear()
        if self._teardown is not None and self.failure is None and self.skip_reason is None:
            try:
                self._teardown()
            except BaseException as exc:  # noqa: BLE001
                errors.append(exc)
        if errors and self.failure is None:
            self.failure = Failure.from_exception(errors[0], paths, phase=Phase.TEARDOWN)
        sink.lifetime_finished(self.id, self.failure, perf_counter() - self._started)


class Lifetimes:
    """The stack of open lifetimes. Before each test, ``enter`` closes the
    open lifetimes the test doesn't share (innermost first) and opens the ones
    it needs (outermost first). A lifetime's span is therefore a property of
    the order, which is the order the runner chose."""

    def __init__(self, sink: Sink, paths: Paths) -> None:
        self.sink = sink
        self.paths = paths
        self.open: list[Lifetime] = []

    def enter(self, needed: Sequence[Lifetime]) -> Lifetime | None:
        """Make ``needed`` (outermost first) the open stack. Returns the
        lifetime whose failure or skip prevents the test from running."""
        shared = 0
        for have, want in zip(self.open, needed, strict=False):
            if have is not want:
                break
            shared += 1
        while len(self.open) > shared:
            self.open.pop().close(self.sink, self.paths)
        for lifetime in self.open:
            if lifetime.failed or lifetime.skip_reason is not None:
                return lifetime
        for lifetime in needed[shared:]:
            self.open.append(lifetime)
            if not lifetime.open(self.sink, self.paths):
                return lifetime
        return None

    def close_all(self) -> None:
        while self.open:
            self.open.pop().close(self.sink, self.paths)

    def report_early(self, blocker: Lifetime | None, maxfail: int | None) -> None:
        """With ``maxfail``, a failed lifetime is closed (its
        ``lifetime_finished`` sent, its failure with it) before the first test
        it blocks finishes, rather than after the last: the runner drops what
        comes after the bad outcome that reaches ``maxfail``, and the tests'
        ``cause`` would name a failure missing from the log. It stays on the
        stack, failed, so the tests it blocks still see it, and closing it
        again does nothing."""
        if maxfail and blocker is not None and blocker.failed:
            blocker.close(self.sink, self.paths)


# ------------------------------------------------------------------ tests


class Test:
    """One test: setup, body, teardown, inside some lifetimes.

    Adapters that map fully subclass it (or pass callables). ``expected``
    holds the reason when the test is expected to fail.
    """

    __test__ = False  # not a pytest test class, wherever it is imported

    def __init__(
        self,
        id: str,
        body: Callable[[], Any] | None = None,
        *,
        lifetimes: Sequence[Lifetime] = (),
        setup: Callable[[], Any] | None = None,
        teardown: Callable[[], Any] | None = None,
        skip: str | None = None,
        expected: str | None = None,
    ) -> None:
        self.id = id
        self._body = body
        self._setup = setup
        self._teardown = teardown
        self.lifetimes = list(lifetimes)
        self.skip = skip
        self.expected = expected

    def setup(self) -> None:
        if self._setup is not None:
            self._setup()

    def body(self) -> None:
        if self._body is not None:
            self._body()

    def teardown(self) -> None:
        if self._teardown is not None:
            self._teardown()


# ------------------------------------------------------------------ batches


class Policy:
    """How a batch must run (``aye_protocol::Policy`` on the Rust side)."""

    def __init__(self, data: dict[str, Any] | None = None) -> None:
        data = data or {}
        self.maxfail: int | None = data.get("maxfail")
        self.capture: bool = data.get("capture", True)
        self.verbosity: int = data.get("verbosity", 0)
        self.timeout: float | None = data.get("timeout")
        self.keyword: str | None = data.get("keyword")
        self.tags: str | None = data.get("tags")
        self.framework_args: list[str] = list(data.get("framework_args", []))


class Batch:
    """A unit of work handed to the engine (``aye_protocol::Batch``)."""

    def __init__(self, data: dict[str, Any]) -> None:
        self.id: int = data["id"]
        self.adapter: str = data["adapter"]
        self.ids: list[str] = list(data["ids"])
        self.policy = Policy(data.get("policy"))
        self.context: Any = data.get("context")
        # The worker's last batch for its adapter: an executor that lists
        # the session ends it with this batch (``aye_protocol::Batch``).
        self.ends_session: bool = bool(data.get("ends_session", False))


# ------------------------------------------------------------------ executors


class Executor:
    """An adapter's runtime face. The worker instantiates one per adapter with
    the project root and the services, then calls ``refine`` and ``run``."""

    adapter = ""
    # Path fragments of the framework's own code, hidden from failure frames.
    internal: Sequence[str] = ()

    def __init__(self, root: str, services: Services) -> None:
        self.paths = Paths(root, self.internal)
        self.root = self.paths.root
        self.services = services

    def refine(self, files: list[str], context: Any) -> dict[str, Any]:
        """Import ``files`` (project-relative) and return their real tests as
        ``{"cases": [...], "diagnostics": [...]}`` in the wire format."""
        raise NotImplementedError(f"the {self.adapter} adapter cannot refine files")

    def run(self, batch: Batch, sink: Sink) -> int | None:
        """Execute ``batch``, emitting events into ``sink``. Returns the
        framework's own status for the batch, if it has one."""
        raise NotImplementedError

    def classify(self, exc: BaseException | None) -> Outcome:
        return classify(exc)


class EngineExecutor(Executor):
    """An executor whose framework maps fully onto aye's engine: the engine
    runs the loop, opening and closing lifetimes as the order requires.
    Subclasses implement ``load``. A test with a bad outcome carries its
    ``reproduce`` (``rerun``)."""

    # The arguments after ``--`` of the batch running now, and aye's own
    # options a rerun repeats (``rerun_args``).
    framework_args: Sequence[str] = ()
    rerun_args: Sequence[str] = ()

    def load(self, batch: Batch) -> list[Test]:
        """Build the batch's tests, in the batch's order."""
        raise NotImplementedError

    def rerun(self, test: Test) -> dict[str, Any] | None:
        """The ``reproduce`` field of a test with a bad outcome: ``aye``,
        aye's options that shaped the run, the id, then ``--`` and the
        batch's arguments after it."""
        return reproduce(test.id, self.framework_args, self.rerun_args)

    def run(self, batch: Batch, sink: Sink) -> int | None:
        self.framework_args = batch.policy.framework_args
        self.rerun_args = rerun_args(batch.context)
        tests = self.load(batch)
        lifetimes = Lifetimes(sink, self.paths)
        context: Context | None = self.services.get(Services.CONTEXT)
        bad = 0
        try:
            for test in tests:
                if batch.policy.maxfail and bad >= batch.policy.maxfail:
                    break
                blocker = lifetimes.enter(test.lifetimes)
                lifetimes.report_early(blocker, batch.policy.maxfail)
                if context is not None:
                    context.test = test.id
                    context.lifetimes = [lt.id for lt in lifetimes.open]
                result = self.run_test(test, blocker, sink)
                if result.outcome is not None and result.outcome.is_bad:
                    bad += 1
        finally:
            lifetimes.close_all()
            if context is not None:
                context.test = None
                context.lifetimes = []
        return 1 if bad else 0

    def run_test(self, test: Test, blocker: Lifetime | None, sink: Sink) -> Result:
        sink.test_started(test.id)
        result = Result(test.id)
        started = perf_counter()
        capture = self.services.get(Services.CAPTURE)
        cleanup: Cleanup | None = self.services.get(Services.CLEANUP)
        try:
            if blocker is not None:
                if blocker.skip_reason is not None:
                    result.add(Outcome.SKIPPED)
                    result.reason = blocker.skip_reason
                else:
                    result.add(Outcome.ERROR)
                    result.cause = blocker.id
            elif test.skip is not None:
                result.add(Outcome.SKIPPED)
                result.reason = test.skip
            else:
                if cleanup is not None:
                    cleanup.push()
                with capture.capture(result) if capture is not None else _nothing():
                    try:
                        self._phases(test, result)
                    finally:
                        if cleanup is not None:
                            self._cleanups(cleanup, result)
        finally:
            result.duration = perf_counter() - started
            if result.outcome is not None and result.outcome.is_bad:
                result.reproduce = _guarded(lambda: self.rerun(test), None)
            sink.test_finished(result)
        return result

    def _phases(self, test: Test, result: Result) -> None:
        """Run setup, body and teardown into ``result``. A ``KeyboardInterrupt``
        is an ``error`` in the phase it hit, recorded before it propagates (a
        body's after its teardown has run), so the test finishes as an error,
        as a test in flight when the session stops does."""
        try:
            test.setup()
        except KeyboardInterrupt as exc:
            result.add(Outcome.ERROR, Failure.from_exception(exc, self.paths, phase=Phase.SETUP))
            raise
        except BaseException as exc:  # noqa: BLE001 - what the test raised is its outcome
            if is_skip(exc):
                result.add(Outcome.SKIPPED)
                result.reason = skip_reason(exc)
            else:
                result.add(
                    Outcome.ERROR, Failure.from_exception(exc, self.paths, phase=Phase.SETUP)
                )
            return
        interrupt: KeyboardInterrupt | None = None
        try:
            test.body()
        except KeyboardInterrupt as exc:
            result.add(Outcome.ERROR, Failure.from_exception(exc, self.paths, phase=Phase.TEST))
            interrupt = exc
        except BaseException as exc:  # noqa: BLE001 - what the test raised is its outcome
            self._body_outcome(test, result, exc)
        else:
            self._body_outcome(test, result, None)
        try:
            test.teardown()
        except KeyboardInterrupt as exc:
            result.add(Outcome.ERROR, Failure.from_exception(exc, self.paths, phase=Phase.TEARDOWN))
            raise
        except BaseException as exc:  # noqa: BLE001 - what the test raised is its outcome
            result.add(Outcome.ERROR, Failure.from_exception(exc, self.paths, phase=Phase.TEARDOWN))
        if interrupt is not None:
            raise interrupt

    def _cleanups(self, cleanup: Cleanup, result: Result) -> None:
        """Close the test's scope on the cleanup service, always, so that an
        interrupt doesn't leave it open."""
        error = cleanup.pop()
        if error is not None:
            result.add(
                Outcome.ERROR, Failure.from_exception(error, self.paths, phase=Phase.TEARDOWN)
            )
            if isinstance(error, KeyboardInterrupt):
                raise error

    def _body_outcome(self, test: Test, result: Result, exc: BaseException | None) -> None:
        outcome = self.classify(exc)
        if test.expected is not None and outcome in (Outcome.FAILED, Outcome.ERROR):
            result.add(Outcome.EXPECTED_FAILURE)
            result.reason = test.expected
            return
        if test.expected is not None and outcome is Outcome.PASSED:
            result.add(Outcome.UNEXPECTED_PASS)
            result.reason = test.expected
            return
        if outcome is Outcome.SKIPPED and exc is not None:
            result.add(outcome)
            result.reason = skip_reason(exc)
            return
        failure = (
            Failure.from_exception(exc, self.paths, phase=Phase.TEST) if exc is not None else None
        )
        result.add(outcome, failure)


@contextmanager
def _nothing() -> Iterator[None]:
    yield


def load_entry(entry: str) -> type[Executor]:
    """Resolve ``module:Class`` among installed modules."""
    module_name, _, class_name = entry.partition(":")
    module = sys.modules.get(module_name) or __import__(module_name)
    return getattr(module, class_name)
