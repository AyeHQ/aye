"""aye's native engine: runs aye's test model (``aye_model``).

A front-end reads a framework's test code into ``aye_model`` objects (cases,
nodes, resources, tags, a policy); this engine runs them and reports through
``aye_engine``'s ``Sink``. It knows no framework: it never imports pytest, a
front-end or the glue that wires them, and it imports only the standard
library, ``aye_engine`` and ``aye_model``. Prototype (October 2026): the
pytest front-end is its first user, so its semantics are pytest's, written
from the specification (``.context/engine-design/map-pytest.md``):

- ``Engine.run(cases, sink)`` runs the cases in the order given. Each case
  has three phases, setup, call and teardown, and one aye outcome folded
  from them (``_fold``): a setup or teardown that raised is ``error``, a
  call that raised is ``failed``, a skip exception is ``skipped``; with an
  ``xfail`` tag in force a raise (matching ``raises``) is
  ``expected_failure`` in any phase and a passing call ``unexpected_pass``
  (``failed`` when strict). The worst phase wins.
- One engine can serve many runs (a worker's batches): with
  ``run(..., keep_session=True)`` the session's lifetime and the last
  case's packages' stay open after the run, their resources alive; the
  next run's first case closes those it doesn't share (their errors, with
  no case to blame, a ``lifetime_finished`` of their own), and ``close()``
  ends the session. Module, class and case lifetimes close with each run,
  and a stop (an exit, an interrupt) closes everything.
- Conditions come first in setup, before any resource or hook: every
  ``skipif`` tag (closest first), then ``skip``, then ``xfail``
  (``run=False`` gives ``expected_failure`` without running anything).
  String conditions are evaluated in the policy's namespace plus the test
  module's globals. A case's tags (its front-end item's marks when it has
  one) are read once, and its xfail verdict evaluated once: again only when
  a tag may have been added since (``tags_changed``, which a front-end
  calls when its marks change, or the item's own marks growing).
- Resources (pytest's fixtures) are resolved through ``case.node.visible``
  (closest definition first; a definition requesting its own name gets the
  next one), then ``builtins``. Each definition holds one live instance at a
  time, keyed by its parameter: the next case reuses it while the parameter
  compares equal and its scope stays open; another parameter tears the old
  instance (and what depends on it) down in that case's setup, as pytest
  does. Failures are cached for the scope and re-raised. Every instance's
  teardown is registered on its scope (the session, the package, the
  module, the class, or the case), and a scope closes in the teardown of
  the last case inside it (``_SetupState.teardown_exact``), finalizers last
  in, first out, so its errors are that case's teardown errors.
- ``Request`` is the ``request`` resources and tests receive; ``Item`` and
  ``NodeView`` are what ``request.node`` returns. A front-end's item gets
  the case's values in its own ``funcargs`` (a value already there is the
  one the test gets), and none once the case's teardown is over
  (``funcargs`` is ``None``), as pytest's items do.
- Output is captured per case into one pair of ``aye_engine.CaptureIO``
  streams, emptied after each case, as pytest's global capture (a stream a
  case kept still captures: what it writes later is the running case's; a
  pair a case closed or took a file descriptor of is replaced); the
  ``capsys``, ``capfd``, ``capsysbinary`` and ``capfdbinary`` builtins take
  it over while they live. Logging is captured per phase (``caplog``,
  ``Result.log``) and warnings filters apply per case (ini lines, ``-W``,
  parsed once per run, then ``filterwarnings`` tags, the outermost
  winning).
- A failed comparison in a rewritten ``assert`` (``exc._aye_assert``,
  from ``aye_rewrite``) becomes the failure's structured ``assertion``, with
  an explanation (``explain``) like pytest's for text, sequences, sets,
  mappings and dataclasses, which side was expected and each side's
  source, and for ``==`` how the values differ (``aye_engine.compare``).
  A failure's frames are described by ``aye_engine.describe_frames`` (the
  span each points at; the innermost project frame's lines before and
  locals). A failure in a setup or teardown names the resource that raised
  it (``resource``), an unknown resource's failure the visible names
  closest to it (``hint``), and a case with a bad outcome lists its
  resources and their scopes (``Result.resources``).
- ``kind="unittest"`` cases run through ``unittest.TestCase.run`` with a
  result that maps to aye outcomes, with ``setUpClass`` and
  ``tearDownClass`` as a class-scoped resource, as pytest runs them.
- ``maxfail`` stops after that many bad outcomes (the last case's teardown
  closes every scope); an exit exception (``policy.exit_exceptions``) or a
  ``KeyboardInterrupt`` stops the run: the case in flight is an ``error``
  and a ``batch-stopped`` diagnostic names the cases that never started.
- ``builtin_resources(config, ...)`` makes ``tmp_path``,
  ``tmp_path_factory``, ``tmpdir``, ``tmpdir_factory``, ``monkeypatch``,
  ``capsys``, ``capfd``, ``capsysbinary``, ``capfdbinary``, ``caplog``,
  ``recwarn``, ``request``, ``pytestconfig``, ``record_property``,
  ``record_testsuite_property``, ``record_xml_attribute``, ``subtests`` and
  ``doctest_namespace``.

Python 3.10 and later.
"""

from __future__ import annotations

import atexit
import builtins
import dataclasses
import fnmatch
import functools
import getpass
import inspect
import io
import itertools
import linecache
import logging
import os
import platform
import re
import reprlib
import shutil
import stat
import sys
import tempfile
import time
import traceback
import unittest
import uuid
import warnings
from collections import namedtuple
from collections.abc import Callable, Iterable, Iterator, Mapping, Sequence
from collections.abc import Set as AbstractSet
from contextlib import contextmanager
from difflib import get_close_matches, ndiff
from pathlib import Path
from pprint import pformat
from time import perf_counter
from typing import Any

import aye_engine
import aye_model
from aye_engine import Failure, Outcome, Phase, Result, Services, Sink
from aye_model import Case, Node, Policy, ResourceDef, Scope, Tag

__all__ = [
    "CaptureFixture",
    "CaptureResult",
    "Engine",
    "Failed",
    "FixtureLookupError",
    "Item",
    "LocalPath",
    "LogCaptureFixture",
    "LogCaptureHandler",
    "NodeView",
    "Request",
    "TeardownErrors",
    "TempPathFactory",
    "XFailed",
    "builtin_resources",
    "explain",
    "saferepr",
]

# Bound at import, as aye_engine binds its own: a test that patches these
# changes nothing the engine reports or how it cleans up.
_format_exception = traceback.format_exception
_format_exception_only = traceback.format_exception_only
_getline = linecache.getline
_dup, _dup2, _close = os.dup, os.dup2, os.close
_environ = os.environ
_isgeneratorfunction = inspect.isgeneratorfunction
_iscoroutinefunction = inspect.iscoroutinefunction
_isasyncgenfunction = inspect.isasyncgenfunction

_NOTSET: Any = object()
_SESSION = ("session",)
_BaseExceptionGroup = getattr(builtins, "BaseExceptionGroup", None)

# The run this worker belongs to and its lane, as the runner named them in
# the environment when it started the worker (read now: a test may change
# the environment). Without a run id the worker is a run of its own.
_RUN = _environ.get("PYTEST_XDIST_TESTRUNUID") or uuid.uuid4().hex
_LANE = _environ.get("PYTEST_XDIST_WORKER") or None
# Marks a ``--basetemp`` directory as emptied by this run already.
_RUN_MARK = ".aye-run"

# The engine's own source files, hidden from failure frames like the
# worker-installed ``<aye:…>`` modules (when the modules are files: tests).
_OWN_FILES = frozenset(
    os.path.normcase(os.path.abspath(path))
    for path in (
        globals().get("__file__"),
        getattr(aye_engine, "__file__", None),
        getattr(aye_model, "__file__", None),
    )
    if isinstance(path, str) and not path.startswith("<")
)

_ASYNC_MESSAGE = (
    "async def functions are not natively supported.\n"
    "You need to install a suitable plugin for your async framework, for example:\n"
    "  - anyio\n"
    "  - pytest-asyncio\n"
    "  - pytest-tornasync\n"
    "  - pytest-trio\n"
    "  - pytest-twisted"
)

_VERDICTS = (Outcome.SKIPPED, Outcome.EXPECTED_FAILURE, Outcome.UNEXPECTED_PASS)

# Each phase as pytest names it (``PYTEST_CURRENT_TEST``, ``caplog``).
_WHEN = {Phase.SETUP: "setup", Phase.TEST: "call", Phase.TEARDOWN: "teardown"}

DEFAULT_LOG_FORMAT = "%(levelname)-8s %(name)s:%(filename)s:%(lineno)d %(message)s"


# ------------------------------------------------------------------ errors


class Failed(Exception):
    """What the engine raises for a failure it detects itself (a scope
    mismatch, a condition that can't be evaluated) when the policy names no
    framework class for it: pytest's ``fail(msg, pytrace=False)``."""

    def __init__(self, msg: str = "", pytrace: bool = True) -> None:
        super().__init__(msg)
        self.msg = msg
        self.pytrace = pytrace


class XFailed(Failed):
    """An expected failure the engine declares itself (``xfail(run=False)``:
    ``[NOTRUN] reason``)."""


class FixtureLookupError(LookupError):
    """A resource that can't be found (or a request that can't be served),
    as pytest words it: ``fixture 'x' not found`` and the visible names.
    For a name defined nowhere the case can see, ``available`` lists the
    names it can (for the failure's hint), else it is ``None``."""

    def __init__(self, argname: str | None, request: Any, msg: str | None = None) -> None:
        self.argname = argname
        self.request = request
        self.msg = msg or ""
        self.available: list[str] | None = None
        super().__init__(self.msg)

    def __str__(self) -> str:
        return self.msg


def _lookup_hint(exc: BaseException) -> str | None:
    """For a resource defined nowhere the case can see, the visible names
    closest to it (``did you mean 'cart'?``, at most three); none when no
    name is close (the message says it is not found)."""
    available = getattr(exc, "available", None) if isinstance(exc, FixtureLookupError) else None
    name = getattr(exc, "argname", None)
    if available is None or not isinstance(name, str):
        return None
    close = get_close_matches(name, available, n=3, cutoff=0.6)
    if not close:
        return None
    quoted = [repr(found) for found in close]
    named = quoted[0] if len(quoted) == 1 else f"{', '.join(quoted[:-1])} or {quoted[-1]}"
    return f"did you mean {named}?"


class TeardownErrors(Exception):
    """Several errors from one teardown, on Python 3.10 (which has no
    ``BaseExceptionGroup``); ``exceptions`` holds them."""

    def __init__(self, message: str, exceptions: Sequence[BaseException]) -> None:
        super().__init__(message)
        self.exceptions = tuple(exceptions)


def _group(message: str, exceptions: Sequence[BaseException]) -> BaseException:
    if _BaseExceptionGroup is not None:
        return _BaseExceptionGroup(message, list(exceptions))
    return TeardownErrors(message, exceptions)


def _is_group(exc: BaseException) -> bool:
    if isinstance(exc, TeardownErrors):
        return True
    return _BaseExceptionGroup is not None and isinstance(exc, _BaseExceptionGroup)


def _leaves(exc: BaseException) -> Iterator[BaseException]:
    """The exceptions inside ``exc``, groups flattened."""
    if _is_group(exc):
        for inner in getattr(exc, "exceptions", ()):
            yield from _leaves(inner)
    else:
        yield exc


def _raise_all(message: str, exceptions: Sequence[BaseException]) -> None:
    if len(exceptions) == 1:
        raise exceptions[0]
    if exceptions:
        raise _group(message, exceptions)


# ------------------------------------------------------------------ config


def _option(config: Any, name: str, default: Any = None) -> Any:
    """``config.getoption(name)``, or ``default`` when there is no config or
    no such option."""
    getoption = getattr(config, "getoption", None)
    if getoption is None:
        return default
    try:
        value = getoption(name, default)
    except Exception:  # noqa: BLE001 - an unknown option is the default
        return default
    return default if value is None else value


def _ini(config: Any, name: str, default: Any = None) -> Any:
    getini = getattr(config, "getini", None)
    if getini is None:
        return default
    try:
        value = getini(name)
    except Exception:  # noqa: BLE001 - an unknown ini key is the default
        return default
    return default if value is None or value == "" else value


def _lines(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        return [line.strip() for line in value.splitlines() if line.strip()]
    try:
        return [str(line) for line in value if str(line).strip()]
    except TypeError:
        return []


def _log_level(value: Any) -> int | None:
    if value is None or value == "":
        return None
    if isinstance(value, int):
        return value
    text = str(value).strip()
    if text.isdigit():
        return int(text)
    level = logging.getLevelName(text.upper())
    return level if isinstance(level, int) else None


# ------------------------------------------------------------------ repr and explanations


class _SafeRepr(reprlib.Repr):
    """``repr`` bounded like pytest's ``saferepr``: containers cut by
    ``reprlib``'s limits, strings and other objects cut to ``maxsize`` in
    the middle, a broken ``__repr__`` shown rather than raised."""

    def __init__(self, maxsize: int | None, use_ascii: bool = False) -> None:
        super().__init__()
        self.maxstring = maxsize if maxsize is not None else sys.maxsize
        self.maxsize = maxsize
        self.use_ascii = use_ascii

    def repr(self, x: object) -> str:
        try:
            text = ascii(x) if self.use_ascii else super().repr(x)
        except (KeyboardInterrupt, SystemExit):
            raise
        except BaseException as exc:  # noqa: BLE001 - a broken __repr__ is the user's
            text = _broken_repr(exc, x)
        return _ellipsize(text, self.maxsize)

    def repr_dict(self, x: dict[Any, Any], level: int) -> str:
        """A dict in its own order (``reprlib`` sorts the keys), at most
        ``maxdict`` items."""
        if not x:
            return "{}"
        if level <= 0:
            return "{...}"
        pieces = [
            f"{self.repr1(key, level - 1)}: {self.repr1(x[key], level - 1)}"
            for key in itertools.islice(x, self.maxdict)
        ]
        if len(x) > self.maxdict:
            pieces.append("...")
        return "{" + ", ".join(pieces) + "}"

    def repr_instance(self, x: object, level: int) -> str:
        try:
            text = repr(x)
        except (KeyboardInterrupt, SystemExit):
            raise
        except BaseException as exc:  # noqa: BLE001
            text = _broken_repr(exc, x)
        return _ellipsize(text, self.maxsize)


def _broken_repr(exc: BaseException, obj: object) -> str:
    try:
        what = repr(exc)
    except Exception:  # noqa: BLE001
        what = type(exc).__name__
    return f"<[{what} raised in repr()] {type(obj).__name__} object at 0x{id(obj):x}>"


def _ellipsize(text: str, maxsize: int | None) -> str:
    if maxsize is None or len(text) <= maxsize:
        return text
    head = max(0, (maxsize - 3) // 2)
    tail = max(0, maxsize - 3 - head)
    return text[:head] + "..." + text[len(text) - tail :]


def saferepr(value: object, maxsize: int | None = 240, use_ascii: bool = False) -> str:
    """``value``'s repr, bounded and never raising (pytest's ``saferepr``)."""
    return _SafeRepr(maxsize, use_ascii).repr(value)


def _on_ci() -> bool:
    return any(name in os.environ for name in ("CI", "BUILD_NUMBER"))


def _is_sequence(value: object) -> bool:
    return isinstance(value, Sequence) and not isinstance(value, str)


def _is_set(value: object) -> bool:
    return isinstance(value, set | frozenset)


def _is_namedtuple(value: object) -> bool:
    return isinstance(value, tuple) and getattr(value, "_fields", None) is not None


def _is_attrs(value: object) -> bool:
    return getattr(value, "__attrs_attrs__", None) is not None


def _is_iterable(value: object) -> bool:
    if isinstance(value, str):
        return False
    try:
        iter(value)  # type: ignore[call-overload]
    except Exception:  # noqa: BLE001 - not iterable after all
        return False
    return True


def _has_generated_eq(value: object) -> bool:
    """Whether ``value``'s ``__eq__`` is the one dataclasses or attrs made
    (a hand-written one may compare anything, so no field diff is shown)."""
    code = getattr(getattr(value, "__eq__", None), "__code__", None)
    filename = getattr(code, "co_filename", None)
    if filename is None:
        return True
    if _is_attrs(value):
        return "attrs generated " in filename
    return filename == "<string>"


def explain(op: str, left: Any, right: Any, verbose: int = 0) -> list[str]:
    """Why ``left op right`` failed, line by line, as pytest's
    ``assertrepr_compare`` explains it below its summary: a text diff, the
    first differing index, the differing items of mappings and sets, the
    differing fields of dataclasses. Empty when there is nothing more to say
    than the summary. Never raises."""
    try:
        if op == "==":
            lines = list(_explain_eq(left, right, verbose))
        elif op == "not in" and isinstance(left, str) and isinstance(right, str):
            lines = list(_explain_not_in_text(left, right, verbose))
        elif op in ("!=", ">=", "<=", ">", "<") and _is_set(left) and _is_set(right):
            lines = list(_explain_set_order(op, left, right))
        else:
            lines = []
    except Exception as exc:  # noqa: BLE001 - a broken __eq__ or __repr__ is the user's
        lines = [
            f"(representation of details failed: {type(exc).__name__}: {exc}.",
            " Probably an object has a faulty __repr__.)",
        ]
    while lines and lines[0] == "":
        lines = lines[1:]
    return lines


def _explain_eq(left: Any, right: Any, verbose: int) -> Iterator[str]:
    if isinstance(left, str) and isinstance(right, str):
        yield from _explain_text(left, right, verbose)
        return
    approx = next(
        (
            (side, other)
            for side, other in ((right, left), (left, right))
            if callable(getattr(side, "_repr_compare", None)) and "pprox" in type(side).__name__
        ),
        None,
    )
    if approx is not None:
        yield from approx[0]._repr_compare(approx[1])
    elif type(left) is type(right) and (
        dataclasses.is_dataclass(left) or _is_attrs(left) or _is_namedtuple(left)
    ):
        yield from _explain_fields(left, right, verbose)
    elif _is_sequence(left) and _is_sequence(right):
        yield from _explain_sequence(left, right)
    elif _is_set(left) and _is_set(right):
        yield from _one_sided("left", left, right)
        yield from _one_sided("right", right, left)
    elif isinstance(left, Mapping) and isinstance(right, Mapping):
        yield from _explain_mapping(left, right, verbose)
    if _is_iterable(left) and _is_iterable(right):
        yield from _explain_full(left, right, verbose)


# A text diff cuts the start (and end) the sides share beyond this many
# characters, keeping ``_CONTEXT`` of them.
_CONTEXT_CUT = 42
_CONTEXT = 10
# The verbosity that shows identical items too (``-vv``).
_VERY_VERBOSE = 2


def _explain_text(left: str, right: str, verbose: int) -> Iterator[str]:
    if verbose < 1:
        # Cut what both share at the start (and, for texts of equal
        # length, at the end), keeping ten characters of context.
        same = 0
        for same in range(min(len(left), len(right))):
            if left[same] != right[same]:
                break
        if same > _CONTEXT_CUT:
            same -= _CONTEXT
            yield f"Skipping {same} identical leading characters in diff, use -v to show"
            left, right = left[same:], right[same:]
        if len(left) == len(right):
            same = 0
            for same in range(len(left)):
                if left[-same] != right[-same]:
                    break
            if same > _CONTEXT_CUT:
                same -= _CONTEXT
                yield f"Skipping {same} identical trailing characters in diff, use -v to show"
                left, right = left[:-same], right[:-same]
    if left.isspace() or right.isspace():
        left, right = repr(str(left)), repr(str(right))
        yield "Strings contain only whitespace, escaping them using repr()"
    # The right side is the expected one the left is compared against.
    diff = (line.strip("\n") for line in ndiff(right.splitlines(True), left.splitlines(True)))
    yield from "\n".join(diff).splitlines()


def _explain_not_in_text(term: str, text: str, verbose: int) -> Iterator[str]:
    index = text.find(term)
    without = text[:index] + text[index + len(term) :]
    yield f"{saferepr(term, maxsize=42)} is contained here:"
    for line in _explain_text(text, without, verbose):
        if line.startswith(("Skipping", "- ")):
            continue
        yield "  " + line[2:] if line.startswith("+ ") else line


def _one_sided(side: str, these: AbstractSet[Any], those: AbstractSet[Any]) -> Iterator[str]:
    extra = these - those
    if extra:
        yield f"Extra items in the {side} set:"
        for item in extra:
            yield saferepr(item)


def _explain_set_order(op: str, left: AbstractSet[Any], right: AbstractSet[Any]) -> Iterator[str]:
    if op == "!=":
        yield "Both sets are equal"
    elif op == ">=":
        yield from _one_sided("right", right, left)
    elif op == "<=":
        yield from _one_sided("left", left, right)
    elif left == right:
        yield "Both sets are equal"
    elif op == ">":
        yield from _one_sided("right", right, left)
    else:
        yield from _one_sided("left", left, right)


def _explain_sequence(left: Sequence[Any], right: Sequence[Any]) -> Iterator[str]:
    both_bytes = isinstance(left, bytes) and isinstance(right, bytes)
    for index in range(min(len(left), len(right))):
        if left[index] != right[index]:
            if both_bytes:
                shown = (left[index : index + 1], right[index : index + 1])
            else:
                shown = (left[index], right[index])
            yield f"At index {index} diff: {shown[0]!r} != {shown[1]!r}"
            break
    if both_bytes:
        return
    extra = len(left) - len(right)
    if extra:
        side, first = (
            ("Left", saferepr(left[len(right)]))
            if extra > 0
            else ("Right", saferepr(right[len(left)]))
        )
        extra = abs(extra)
        if extra == 1:
            yield f"{side} contains one more item: {first}"
        else:
            yield f"{side} contains {extra} more items, first extra item: {first}"


def _explain_mapping(
    left: Mapping[Any, Any], right: Mapping[Any, Any], verbose: int
) -> Iterator[str]:
    left_keys, right_keys = set(left), set(right)
    common = left_keys & right_keys
    same = {key: left[key] for key in common if left[key] == right[key]}
    if same and verbose < _VERY_VERBOSE:
        yield f"Omitting {len(same)} identical items, use -vv to show"
    elif same:
        yield "Common items:"
        yield from pformat(same).splitlines()
    differing = {key for key in common if left[key] != right[key]}
    if differing:
        yield "Differing items:"
        for key in differing:
            yield saferepr({key: left[key]}) + " != " + saferepr({key: right[key]})
    for side, these, those, mapping in (
        ("Left", left_keys, right_keys, left),
        ("Right", right_keys, left_keys, right),
    ):
        extra = these - those
        if extra:
            yield f"{side} contains {len(extra)} more item{'' if len(extra) == 1 else 's'}:"
            yield from pformat({key: mapping[key] for key in extra}).splitlines()


def _explain_fields(left: Any, right: Any, verbose: int) -> Iterator[str]:
    if not _has_generated_eq(left):
        return
    if dataclasses.is_dataclass(left):
        names = [f.name for f in dataclasses.fields(left) if f.compare]
    elif _is_attrs(left):
        names = [f.name for f in left.__attrs_attrs__ if getattr(f, "eq", True)]
    else:
        names = list(left._fields)
    same = [name for name in names if getattr(left, name) == getattr(right, name)]
    differing = [name for name in names if name not in same]
    if same or differing:
        yield ""
    if same and verbose < _VERY_VERBOSE:
        yield f"Omitting {len(same)} identical items, use -vv to show"
    elif same:
        yield "Matching attributes:"
        yield from pformat(same).splitlines()
    if differing:
        yield "Differing attributes:"
        yield from pformat(differing).splitlines()
        for name in differing:
            here, there = getattr(left, name), getattr(right, name)
            yield ""
            yield f"Drill down into differing attribute {name}:"
            yield f"  {name}: {here!r} != {there!r}"
            for line in _explain_eq(here, there, verbose):
                yield "  " + line


def _explain_full(left: Any, right: Any, verbose: int) -> Iterator[str]:
    if verbose <= 0 and not _on_ci():
        yield "Use -v to get more diff"
        return
    left_lines = _expanded(left).splitlines()
    right_lines = _expanded(right).splitlines()
    yield ""
    yield "Full diff:"
    yield from (line.rstrip() for line in ndiff(right_lines, left_lines))


def _expanded(value: Any, indent: int = 0, seen: frozenset[int] = frozenset()) -> str:  # noqa: PLR0911 - one return per case
    """``value`` with one item per line, each followed by a comma, nested
    containers indented by four: how pytest prints the operands of a full
    diff, so that the diff shows the items that differ."""
    if id(value) in seen:
        return f"<Recursion on {type(value).__name__} with id={id(value)}>"
    seen = seen | {id(value)}
    inner = " " * (indent + 4)
    kind = type(value).__repr__

    def items(parts: Iterable[str], opening: str, closing: str) -> str:
        lines = [f"\n{inner}{part}," for part in parts]
        if not lines:
            return opening + closing
        return opening + "".join(lines) + "\n" + " " * indent + closing

    if kind is dict.__repr__:
        return items(
            (f"{_short_repr(k)}: {_expanded(v, indent + 4, seen)}" for k, v in value.items()),
            "{",
            "}",
        )
    if kind in (list.__repr__, tuple.__repr__):
        opening, closing = ("[", "]") if isinstance(value, list) else ("(", ")")
        return items((_expanded(v, indent + 4, seen) for v in value), opening, closing)
    if kind in (set.__repr__, frozenset.__repr__):
        if not value:
            return repr(value)
        ordered = sorted(value, key=_SortKey)
        opening, closing = (
            ("{", "}") if type(value) is set else (f"{type(value).__name__}({{", "})")
        )
        return items((_expanded(v, indent + 4, seen) for v in ordered), opening, closing)
    wrapped = getattr(type(value).__repr__, "__wrapped__", None)
    if (
        dataclasses.is_dataclass(value)
        and not isinstance(value, type)
        and "__create_fn__" in getattr(wrapped, "__qualname__", "")
    ):
        fields = [f for f in dataclasses.fields(value) if f.repr]
        parts = (
            f"{f.name}={_expanded(getattr(value, f.name), indent + 4 + len(f.name) + 1, seen)}"
            for f in fields
        )
        return items(parts, type(value).__name__ + "(", ")")
    return _short_repr(value)


def _short_repr(value: Any) -> str:
    try:
        return repr(value)
    except Exception as exc:  # noqa: BLE001 - a broken __repr__ is the user's
        return _broken_repr(exc, value)


class _SortKey:
    """Orders mixed values as pprint does: by value when they compare, else
    by type name and id."""

    __slots__ = ("value",)

    def __init__(self, value: Any) -> None:
        self.value = value

    def __lt__(self, other: _SortKey) -> bool:
        try:
            return bool(self.value < other.value)
        except TypeError:
            return (str(type(self.value)), id(self.value)) < (
                str(type(other.value)),
                id(other.value),
            )


# ------------------------------------------------------------------ marks


def _as_tag(marker: Any) -> Tag:
    """A tag from what ``add_marker`` accepts: a name, a mark, a mark
    decorator (whose ``.mark`` is the mark) or a tag."""
    if isinstance(marker, Tag):
        return marker
    if isinstance(marker, str):
        return Tag(marker)
    mark = getattr(marker, "mark", marker)
    name = getattr(mark, "name", None)
    if not isinstance(name, str):
        raise ValueError("is not a string or pytest.mark.* Marker")  # noqa: TRY004 - pytest's
    return Tag(
        name, tuple(getattr(mark, "args", ()) or ()), dict(getattr(mark, "kwargs", {}) or {})
    )


@dataclasses.dataclass(frozen=True)
class _Xfail:
    reason: str
    run: bool
    strict: bool
    raises: Any


# ------------------------------------------------------------------ views


def _node_key(node: Node) -> tuple[str, str, str]:
    return ("node", node.kind, node.id)


def _kept(keys: Sequence[tuple]) -> list[tuple]:
    """The lifetimes among ``keys`` (outermost first) that outlive a run
    with ``keep_session``: the session's, then the packages'."""
    kept = []
    for key in keys:
        if key != _SESSION and key[:2] != ("node", "package"):
            break
        kept.append(key)
    return kept


def _last_segment(nodeid: str) -> str:
    """The node's own name in its id: ``test_y[1-a]`` in
    ``tests/test_a.py::TestX::test_y[1-a]``, ``test_a.py`` in
    ``tests/test_a.py``."""
    head, bracket, params = nodeid.partition("[")
    if "::" in head:
        name = head.rsplit("::", 1)[-1]
    else:
        name = head.rstrip("/").rsplit("/", 1)[-1]
    return name + bracket + params


class _Keywords(dict):
    """A node's keywords: names that ``-k`` matches, marks among them."""


class NodeView:
    """``request.node`` for a resource whose scope is a package, a module, a
    class (or the session): the name, the id, the tags of the node and its
    parents, and ``addfinalizer`` on the node's scope. Other attributes are
    read from the front-end's collector for the same object, when the case
    running has a front-end item whose parents include one."""

    def __init__(
        self, engine: Engine, key: tuple, node: Node | None, parent: NodeView | None
    ) -> None:
        self._engine = engine
        self._key = key
        self._node = node
        self.parent = parent
        self.nodeid = node.id if node is not None else ""
        self.name = _last_segment(self.nodeid) if node is not None else ""
        self.obj = node.obj if node is not None else None
        self.stash: dict[Any, Any] = {}

    def __repr__(self) -> str:
        kind = self._node.kind.capitalize() if self._node is not None else "Session"
        return f"<{kind} {self.name}>"

    def __getattr__(self, name: str) -> Any:
        if name.startswith("__") or "_engine" not in self.__dict__:
            raise AttributeError(name)
        collector = self._collector()
        if collector is not None:
            return getattr(collector, name)
        raise AttributeError(f"{type(self).__name__!r} object has no attribute {name!r}")

    def _collector(self) -> Any:
        """The front-end's node for the same object: a parent of the running
        case's front-end item whose ``obj`` is this node's."""
        state = self._engine._current
        node: Any = getattr(state, "front_item", None)
        while node is not None:
            node = getattr(node, "parent", None)
            if node is not None and self._node is None and getattr(node, "parent", 1) is None:
                return node  # the session
            if (
                node is not None
                and self._node is not None
                and getattr(node, "obj", None) is self.obj
            ):
                return node
        return None

    @property
    def config(self) -> Any:
        return self._engine.config

    @property
    def session(self) -> Any:
        state = self._engine._current
        return state.session if state is not None else self._engine.session

    @property
    def cls(self) -> type | None:
        for node in self.listchain()[::-1]:
            kind = node._node.kind if isinstance(node, NodeView) and node._node is not None else ""
            if kind == "class":
                return node.obj
        return None

    @property
    def module(self) -> Any:
        for node in self.listchain()[::-1]:
            kind = node._node.kind if isinstance(node, NodeView) and node._node is not None else ""
            if kind == "module":
                return node.obj
        return None

    @property
    def path(self) -> Path | None:
        obj = self.obj
        file = getattr(obj, "__file__", None)
        if file is None and obj is not None:
            module = sys.modules.get(getattr(obj, "__module__", "") or "")
            file = getattr(module, "__file__", None)
        return Path(file) if file else None

    @property
    def fspath(self) -> LocalPath | None:
        path = self.path
        return LocalPath(path) if path is not None else None

    @property
    def own_markers(self) -> list[Tag]:
        added = self._engine._added_tags.get(self._key, [])
        return list(self._node.tags if self._node is not None else []) + added

    def listchain(self) -> list[Any]:
        chain: list[Any] = []
        node: Any = self
        while node is not None:
            chain.append(node)
            node = node.parent
        return chain[::-1]

    def iter_markers(self, name: str | None = None) -> Iterator[Tag]:
        for node in reversed(self.listchain()):
            for tag in node.own_markers:
                if name is None or tag.name == name:
                    yield tag

    def iter_markers_with_node(self, name: str | None = None) -> Iterator[tuple[Any, Tag]]:
        for node in reversed(self.listchain()):
            for tag in node.own_markers:
                if name is None or tag.name == name:
                    yield node, tag

    def get_closest_marker(self, name: str, default: Any = None) -> Any:
        return next(self.iter_markers(name), default)

    def add_marker(self, marker: Any, append: bool = True) -> None:
        tag = _as_tag(marker)
        added = self._engine._added_tags.setdefault(self._key, [])
        if append:
            added.append(tag)
        else:
            added.insert(0, tag)
        self._engine.tags_changed()

    @property
    def keywords(self) -> _Keywords:
        keywords = _Keywords()
        for node in self.listchain():
            if node.name:
                keywords[node.name] = True
            for tag in node.own_markers:
                keywords[tag.name] = tag
        return keywords

    def addfinalizer(self, fin: Callable[[], object]) -> None:
        self._engine._setupstate.addfinalizer(fin, self._key)

    def getparent(self, cls: type) -> Any:
        node: Any = self
        while node is not None and not isinstance(node, cls):
            node = node.parent
        return node

    def warn(self, warning: Warning) -> None:
        warnings.warn(warning, stacklevel=2)


class Item(NodeView):
    """``request.node`` in a test and in its function-scoped resources: the
    case as pytest's ``Item`` shows it. Attributes the front-end's own item
    object (``case.extra["item"]``) has and this one doesn't are read from
    it."""

    def __init__(self, engine: Engine, state: _CaseState) -> None:
        case = state.case
        parent = engine._view_of_node(case.node)
        super().__init__(engine, state.key, None, parent)
        self._state = state
        self._case = case
        self.nodeid = case.id
        self.name = _last_segment(case.id)
        self.originalname = self.name.partition("[")[0]
        self.user_properties: list[tuple[str, object]] = []
        self.location = (case.location[0], max(case.location[1] - 1, 0), self.name)
        self.funcargs = state.funcargs
        self.extra_keyword_matches: set[str] = set()
        params = {**case.call_params, **{k: v[1] for k, v in case.resource_params.items()}}
        if params:
            indices = {k: v[0] for k, v in case.resource_params.items()}
            self.callspec = _CallSpec(params, indices, self.name.partition("[")[2][:-1])

    def __repr__(self) -> str:
        kind = "TestCaseFunction" if self._case.kind == "unittest" else "Function"
        return f"<{kind} {self.name}>"

    def __getattr__(self, name: str) -> Any:
        other = self.__dict__.get("_case")
        item = other.extra.get("item") if other is not None else None
        if item is not None and item is not self and not name.startswith("__"):
            return getattr(item, name)
        raise AttributeError(f"{type(self).__name__!r} object has no attribute {name!r}")

    @property
    def obj(self) -> Any:  # type: ignore[override]
        return self._state.test_callable()

    @obj.setter
    def obj(self, value: Any) -> None:
        pass

    @property
    def function(self) -> Any:
        return self._case.func

    @property
    def cls(self) -> type | None:
        return self._case.cls

    @property
    def instance(self) -> Any:
        return self._state.instance

    @property
    def module(self) -> Any:
        return self._state.module

    @property
    def path(self) -> Path | None:  # type: ignore[override]
        return self._state.path

    @property
    def fixturenames(self) -> list[str]:
        return self._state.fixturenames

    @property
    def own_markers(self) -> list[Tag]:  # type: ignore[override]
        return list(self._state.added_tags)

    def iter_markers(self, name: str | None = None) -> Iterator[Tag]:
        for tag in self._state.tags():
            if name is None or tag.name == name:
                yield tag

    def iter_markers_with_node(self, name: str | None = None) -> Iterator[tuple[Any, Tag]]:
        for tag in self.iter_markers(name):
            yield self, tag

    def add_marker(self, marker: Any, append: bool = True) -> None:
        tag = _as_tag(marker)
        if append:
            self._state.added_tags.append(tag)
        else:
            self._state.added_tags.insert(0, tag)
        self._engine.tags_changed()

    @property
    def keywords(self) -> _Keywords:
        keywords = _Keywords({name: True for name in self._case.keywords})
        keywords[self.name] = True
        for tag in self.iter_markers():
            keywords.setdefault(tag.name, tag)
        keywords.update({name: True for name in self.extra_keyword_matches})
        return keywords

    def listextrakeywords(self) -> set[str]:
        return set(self.extra_keyword_matches)

    def add_report_section(self, when: str, key: str, content: str) -> None:
        """Accepted and dropped: aye reports output per test."""

    def reportinfo(self) -> tuple[Any, int, str]:
        return self.path, self.location[1], self.name


class _CallSpec:
    def __init__(self, params: dict[str, Any], indices: dict[str, int], id: str) -> None:
        self.params = params
        self.indices = indices
        self.id = id

    def getparam(self, name: str) -> Any:
        try:
            return self.params[name]
        except KeyError:
            raise ValueError(name) from None


class _Session:
    """``request.session``: the run's flags a conftest may set to stop it."""

    def __init__(self, engine: Engine) -> None:
        self._engine = engine
        self.shouldstop: bool | str = False
        self.shouldfail: bool | str = False
        self.testsfailed = 0
        self.testscollected = 0
        self.name = ""
        self.nodeid = ""

    def reset(self, collected: int) -> None:
        """A new run: no stop asked for, nothing failed yet."""
        self.shouldstop = self.shouldfail = False
        self.testsfailed = 0
        self.testscollected = collected

    @property
    def config(self) -> Any:
        return self._engine.config

    @property
    def items(self) -> list[Any]:
        items = (self._engine.item(case) for case in self._engine._cases)
        return [item for item in items if item is not None]


# ------------------------------------------------------------------ requests


class Request:
    """The ``request`` a test or a resource receives (pytest's
    ``FixtureRequest``, ``TopRequest`` and ``SubRequest``): what is being
    set up (``fixturename``, ``param``, ``scope``), for which test
    (``node``, ``function``, ``cls``, ``instance``, ``module``, ``path``,
    ``config``), and the calls a resource makes back (``getfixturevalue``,
    ``addfinalizer``, ``applymarker``)."""

    def __init__(
        self,
        engine: Engine,
        state: _CaseState,
        *,
        fixturename: str | None = None,
        definition: ResourceDef | None = None,
        scope: Scope = Scope.FUNCTION,
        param: Any = _NOTSET,
        param_index: int = 0,
        parent: Request | None = None,
    ) -> None:
        self._engine = engine
        self._state = state
        self._definition = definition
        self._parent_request = parent
        self._scope = scope
        self.fixturename = fixturename
        if param is not _NOTSET:
            self.param = param
        self.param_index = param_index
        self._key = engine._scope_key(state, scope, definition)

    def __repr__(self) -> str:
        if self.fixturename is None:
            return f"<FixtureRequest for {self._state.item!r}>"
        return f"<SubRequest {self.fixturename!r} for {self._state.item!r}>"

    @property
    def scope(self) -> str:
        return self._scope.name.lower()

    @property
    def node(self) -> Any:
        return self._engine._view_of_key(self._state, self._key)

    @property
    def config(self) -> Any:
        return self._engine._config_of(self._state.case)

    @property
    def session(self) -> Any:
        return self._state.session

    @property
    def function(self) -> Any:
        if self._scope is not Scope.FUNCTION:
            raise AttributeError(f"function not available in {self.scope}-scoped context")
        return self._state.test_callable()

    @property
    def cls(self) -> type | None:
        if self._scope not in (Scope.CLASS, Scope.FUNCTION):
            raise AttributeError(f"cls not available in {self.scope}-scoped context")
        return self._state.case.cls

    @property
    def instance(self) -> Any:
        if self._scope is not Scope.FUNCTION:
            return None
        return self._state.instance

    @property
    def module(self) -> Any:
        if self._scope not in (Scope.FUNCTION, Scope.CLASS, Scope.MODULE):
            raise AttributeError(f"module not available in {self.scope}-scoped context")
        return self._state.module

    @property
    def path(self) -> Path | None:
        if self._scope is Scope.SESSION:
            raise AttributeError("path not available in session-scoped context")
        return self._state.path

    @property
    def fspath(self) -> LocalPath | None:
        path = self.path
        return LocalPath(path) if path is not None else None

    @property
    def keywords(self) -> Any:
        return self.node.keywords

    @property
    def fixturenames(self) -> list[str]:
        return list(self._state.fixturenames)

    @property
    def _pyfuncitem(self) -> Item:
        return self._state.item

    @property
    def _fixturemanager(self) -> Any:
        return None

    def addfinalizer(self, finalizer: Callable[[], object]) -> None:
        """Run ``finalizer`` when this resource's instance is torn down (or,
        for a test's own request, at the test's teardown)."""
        if self._definition is None:
            self._engine._setupstate.addfinalizer(finalizer, self._state.key)
        else:
            self._engine._slot(self._definition).finalizers.append(finalizer)

    def applymarker(self, marker: Any) -> None:
        self.node.add_marker(marker)

    def raiseerror(self, msg: str | None) -> None:
        raise FixtureLookupError(None, self, msg)

    def getfixturevalue(self, argname: str) -> Any:
        """The value of the resource ``argname``, set up now if needed."""
        return self._engine._value(self, argname)

    def _requests(self) -> Iterator[Request]:
        """This request and its parents, up to (not including) the test's."""
        request: Request | None = self
        while request is not None and request.fixturename is not None:
            yield request
            request = request._parent_request


# ------------------------------------------------------------------ resources and scopes


class _Slot:
    """One resource definition's live instance (pytest's ``FixtureDef``
    state): ``cached`` is ``(value, key, exception)`` or ``None``."""

    __slots__ = ("cached", "definition", "finalizers")

    def __init__(self, definition: ResourceDef) -> None:
        self.definition = definition
        self.cached: tuple[Any, Any, BaseException | None] | None = None
        self.finalizers: list[Callable[[], object]] = []


class _SetupState:
    """The open scopes, outermost first, each with its finalizers (pytest's
    ``SetupState``): a case opens the ones it needs at setup, and its
    teardown closes those the next case doesn't share."""

    def __init__(self) -> None:
        self.stack: dict[tuple, list[Callable[[], object]]] = {}

    def setup(self, keys: Sequence[tuple]) -> None:
        """Open the scopes ``keys`` (outermost first) that aren't open. Any
        open scope the case doesn't share (left by an engine error) closes
        first, and its errors are the case's."""
        errors = self.teardown_exact(keys)
        for key in keys:
            self.stack.setdefault(key, [])
        _raise_all("errors during test setup", errors)

    def addfinalizer(self, fin: Callable[[], object], key: tuple) -> None:
        if key in self.stack:
            self.stack[key].append(fin)
        elif self.stack:
            # A scope that isn't open (a request served after its node
            # closed): the innermost open one runs it.
            next(reversed(self.stack.values())).append(fin)
        else:
            self.stack[key] = [fin]

    def teardown_exact(self, needed: Sequence[tuple]) -> list[BaseException]:
        """Close the scopes ``needed`` (the next case's, or none) doesn't
        share, innermost first, each one's finalizers last in, first out.
        Returns what they raised, one entry per scope (a group when a scope
        raised several)."""
        errors: list[BaseException] = []
        while self.stack:
            keys = list(self.stack)
            if keys == list(needed[: len(keys)]):
                break
            key, finalizers = self.stack.popitem()
            raised: list[BaseException] = []
            while finalizers:
                fin = finalizers.pop()
                try:
                    fin()
                except BaseException as exc:  # noqa: BLE001 - every finalizer must run
                    raised.append(exc)
            if len(raised) == 1:
                errors.append(raised[0])
            elif raised:
                errors.append(_group(f"errors while tearing down {key[-1]}", raised[::-1]))
        return errors


# ------------------------------------------------------------------ per case


class _CaseState:
    """What the engine keeps about the case it is running."""

    def __init__(self, engine: Engine, index: int, case: Case) -> None:
        self.engine = engine
        self.index = index
        self.case = case
        self.key = ("case", index)
        self.keys = [*engine._node_keys(case.node), self.key]
        self.funcargs: dict[str, Any] = {}
        self.fixture_defs: dict[str, _Slot] = {}
        self.closure = engine._closure(case)
        self.fixturenames = list(self.closure)
        self.instance: Any = None
        self.xfailed: _Xfail | None = None
        self.added_tags: list[Tag] = []
        self.setup_ok = False
        self.call_ok = False
        self.capture_fixture: CaptureFixture | None = None
        self.failed_subtests = 0
        self.unittest_errors: list[BaseException] = []
        self.log_records: dict[str, list[logging.LogRecord]] = {}
        self.filter_error: BaseException | None = None
        # The tags and the xfail verdict as last computed, with the tags'
        # version then (``tags_version``).
        self._tags: list[Tag] | None = None
        self._tags_version: tuple[int, int] | None = None
        self._xfail: _Xfail | None = None
        self._xfail_version: tuple[int, int] | None = None
        # The front-end's own item for the case, when it made one (pytest's
        # hooks receive it): ``request.node`` is then that object, so that a
        # hook and a resource see one item, its marks included.
        self.front_item = _item_like(case.extra.get("item"))
        marks = getattr(self.front_item, "own_markers", None)
        self._front_marks: list[Any] | None = marks if isinstance(marks, list) else None
        self.item: Any = self.front_item if self.front_item is not None else self.own_item
        if self.front_item is not None:
            # pytest's item fills the values into its own ``funcargs`` (a new
            # one when a run before this one ended with them gone).
            front = self.front_item
            found = aye_engine._guarded(lambda: getattr(front, "funcargs", None), None)
            if type(found) is dict:
                self.funcargs = found
            else:
                aye_engine._guarded(lambda: setattr(front, "funcargs", self.funcargs), None)
        self.session = case.extra.get("session") or engine.session
        self.top = Request(engine, self)

    @functools.cached_property
    def own_item(self) -> Item:
        """The engine's item for the case (``request.node`` when the
        front-end made none)."""
        return Item(self.engine, self)

    @functools.cached_property
    def originalname(self) -> str:
        """The test's name without its parameters."""
        return _last_segment(self.case.id).partition("[")[0]

    @functools.cached_property
    def module(self) -> Any:
        for node in reversed(self.case.node.chain()):
            if node.kind == "module":
                return node.obj
        return sys.modules.get(getattr(self.case.func, "__module__", "") or "")

    @functools.cached_property
    def path(self) -> Path | None:
        file = getattr(self.module, "__file__", None)
        if file:
            return Path(file)
        if self.case.location[0]:
            return Path(self.engine.paths.root) / self.case.location[0]
        return None

    def tags(self) -> list[Tag]:
        """Every tag on the case, closest first, with those added while
        running (to the case first, to its nodes after its own). With the
        front-end's item, its marks are the case's: a hook may add some.
        Computed once, and again only when ``tags_version`` changed."""
        version = self.tags_version()
        if self._tags is None or self._tags_version != version:
            self._tags = self._compute_tags()
            self._tags_version = version
        return self._tags

    def tags_version(self) -> tuple[int, int]:
        """Changes when a tag may have been added: the engine's epoch
        (``Engine.tags_changed``), and how many marks the front-end's item
        holds itself (for a front-end that doesn't say when they change)."""
        marks = self._front_marks
        return self.engine._tags_epoch, len(marks) if marks is not None else -1

    def _compute_tags(self) -> list[Tag]:
        if self.front_item is not None:
            marks = aye_engine._guarded(lambda: list(self.front_item.iter_markers()), None)
            if marks is not None:
                tag_of = self.engine._tag_of
                own = [tag_of(mark) for mark in marks if getattr(mark, "name", None)]
                tags = [*self.added_tags, *own]
            else:
                tags = [*self.added_tags, *self.case.tags]
        else:
            tags = [*self.added_tags, *self.case.tags]
        added = self.engine._added_tags
        if added:
            for key in reversed(self.keys[:-1]):
                tags.extend(added.get(key, ()))
        return tags

    def test_callable(self) -> Any:
        """The test as the framework calls it: bound to the case's instance
        for a class's test (read from the instance, as pytest reads it, so a
        ``staticmethod`` stays unbound)."""
        case = self.case
        if case.cls is None or self.instance is None:
            return case.func
        name = self.originalname
        if self.engine._has_static(self.instance, name):
            return getattr(self.instance, name)
        func = case.func
        if inspect.isfunction(func):
            return func.__get__(self.instance, case.cls)
        return func


# ------------------------------------------------------------------ the engine


class Engine:
    """Runs cases of aye's model. ``hooks`` are called around each case,
    ``builtins`` are resources visible everywhere (behind every definition
    of the same name the front-end made visible). ``config`` is the
    framework's configuration object, handed to resources as
    ``request.config`` (else ``case.extra["config"]``) and read for the
    settings the engine applies (``verbose``, ``runxfail``, ``log_level``,
    ``log_format``, ``filterwarnings``, ``pythonwarnings``).
    ``current_test_env`` names an environment variable set to
    ``"<id> (setup|call|teardown)"`` while a case runs (pytest's
    ``PYTEST_CURRENT_TEST``); ``return_warning`` is the warning class for a
    test returning something other than ``None``; ``disabled`` names
    built-in behaviours turned off (``"logging"``, ``"warnings"``)."""

    def __init__(  # noqa: PLR0913 - keyword-only options
        self,
        paths: aye_engine.Paths,
        services: Services,
        policy: Policy,
        hooks: aye_model.Hooks | None = None,
        builtins: dict[str, ResourceDef] | None = None,
        *,
        config: Any = None,
        current_test_env: str | None = None,
        return_warning: type[Warning] | None = None,
        disabled: Iterable[str] = (),
    ) -> None:
        self.paths = paths
        self.services = services
        self.policy = policy
        self.hooks = hooks or aye_model.Hooks()
        self.builtins = dict(builtins or {})
        self.config = config
        self.current_test_env = current_test_env
        self.return_warning = return_warning
        self.disabled = frozenset(disabled)
        self.verbose = int(_option(config, "verbose", 0) or 0)
        self.runxfail = bool(_option(config, "runxfail", False))
        self.session = _Session(self)
        self.sink: Sink = aye_engine.ListSink()
        self._cases: list[Case] = []
        self._slots: dict[ResourceDef, _Slot] = {}
        self._setupstate = _SetupState()
        self._added_tags: dict[tuple, list[Tag]] = {}
        self._views: dict[tuple, NodeView] = {}
        self._items: dict[int, Item] = {}
        self._class_instances: dict[type, Any] = {}
        self._unittest_defs: dict[type, ResourceDef] = {}
        self._outer_streams: tuple[Any, Any] | None = None
        # The capture streams the next case writes into: one pair for the
        # session, emptied after each case, as pytest's global capture (a
        # pair a case closed, or gave a file descriptor of, is replaced).
        self._streams: tuple[Any, Any] | None = None
        self._capture_fixture: CaptureFixture | None = None
        self._current: _CaseState | None = None
        self._log = _LogCapture(config) if "logging" not in self.disabled else None
        # Bumped whenever a tag is added anywhere (``tags_changed``): a
        # case's tags and xfail verdict are computed again only then.
        self._tags_epoch = 0
        # Whether the session's and the packages' lifetimes outlive a run.
        self._keep = False
        # The warnings filters of the configuration (ini lines, ``-W``),
        # parsed once per run, by configuration; ``filterwarnings`` tags'
        # lines, parsed once.
        self._config_filters: dict[int, tuple[list[tuple[Any, ...]], BaseException | None]] = {}
        self._tag_filters: dict[str, tuple[Any, ...]] = {}
        # Computed once per object: a node's lifetime keys (with the node),
        # a mark's tag (with the mark), whether a test function is async.
        self._chains: dict[int, tuple[Node, list[tuple]]] = {}
        self._mark_tags: dict[int, tuple[Any, Tag]] = {}
        self._async: dict[Any, bool] = {}
        self._static: dict[tuple[type, str], bool] = {}
        # What a resource's setup or teardown raised, with its name (the
        # failure's ``resource``): kept until the case's result is built.
        self._raised_by: list[tuple[BaseException, str]] = []
        xfail_types = tuple(policy.xfail_exceptions)
        self._xfail_types = (*xfail_types, XFailed)
        self._skip_types = tuple(policy.skip_exceptions)
        self._exit_types = tuple(policy.exit_exceptions)
        self._fail_class: type[BaseException] = (
            policy.fail_exceptions[0] if policy.fail_exceptions else Failed
        )

    # ------------------------------------------------------------ the loop

    def run(
        self,
        cases: list[Case],
        sink: Sink,
        *,
        maxfail: int = 0,
        capture: bool = True,
        reproduce: Callable[[str], dict[str, Any] | None] | None = None,
        keep_session: bool = False,
    ) -> int:
        """Run ``cases`` in order, reporting into ``sink``. Returns 0, 1 when
        a case had a bad outcome, or the exit status a stop asked for (2
        for an interrupt or an exit without one).

        Every lifetime closes by the end of the run, unless
        ``keep_session``: then the session's and the last case's packages'
        stay open (their resources alive) for the next ``run``, whose first
        case closes those it doesn't share, and for ``close``. A stop (an
        exit, an interrupt) closes everything all the same."""
        self.sink = sink
        self._cases = list(cases)
        self._keep = keep_session
        self.session.reset(len(self._cases))
        self._items = {}
        self._config_filters = {}
        context = self.services.get(Services.CONTEXT)
        bad = 0
        stop: tuple[str, int] | None = None
        started = 0
        try:
            if self._setupstate.stack:
                # Left open by the previous run: what the first case doesn't
                # share closes now, with no case to blame.
                needed = self._chain_keys(self._cases[0], 0) if self._cases else []
                self._close_scopes(needed, sink)
            for index, case in enumerate(self._cases):
                following = self._cases[index + 1] if index + 1 < len(self._cases) else None
                left = maxfail - bad if maxfail else 0
                result, stop = self._run_case(index, case, following, left, capture, reproduce)
                started += 1
                if result.outcome is not None and result.outcome.is_bad:
                    bad += 1
                    self.session.testsfailed += 1
                if stop is not None:
                    break
                if maxfail and bad >= maxfail:
                    break
                session = case.extra.get("session") or self.session
                if getattr(session, "shouldstop", False) or getattr(session, "shouldfail", False):
                    stop = _session_stop(session)
                    break
        finally:
            self._close_leftovers(stopped=stop is not None)
            if context is not None:
                context.test = None
                context.lifetimes = []
        if stop is not None:
            why, status = stop
            self._stopped(why, status, started)
            return status
        return 1 if bad else 0

    def item(self, case: Case) -> Item | None:
        """The item ``request.node`` shows for ``case`` while it runs (for a
        front-end's hooks)."""
        current = self._current
        if current is not None and current.case is case:
            return current.item
        return self._items.get(id(case))

    def _stopped(self, why: str, status: int, started: int) -> None:
        unstarted = [case.id for case in self._cases[started:]]
        total = len(self._cases)
        if unstarted:
            named = ", ".join(unstarted[:20])
            more = len(unstarted) - 20
            listed = f"{named} and {more} more" if more > 0 else named
            what = f"{len(unstarted)} of {total} tests in the batch never started: {listed}"
        else:
            what = "every test in the batch started"
        message = f"the session stopped with exit status {status}: {why}; {what}"
        self.sink.diagnostic("error", "batch-stopped", "", message)

    def close(self, sink: Sink | None = None) -> list[Failure]:
        """End the session: close every lifetime still open (the session's
        and the packages' that ``keep_session`` kept). With no case to
        blame, their errors are a ``lifetime_finished`` of their own, into
        ``sink`` when there is one; they are returned either way."""
        self._keep = False
        return self._close_scopes([], sink)

    def _tag_of(self, mark: Any) -> Tag:
        """``mark`` as a tag (``_as_tag``), once per mark object."""
        found = self._mark_tags.get(id(mark))
        if found is None or found[0] is not mark:
            found = self._mark_tags[id(mark)] = (mark, _as_tag(mark))
        return found[1]

    def _has_static(self, instance: Any, name: str) -> bool:
        """Whether ``instance`` has ``name`` without running a
        ``__getattr__`` (``inspect.getattr_static``); a class's answer is
        kept."""
        try:
            if name in vars(instance):
                return True
        except TypeError:
            pass
        key = (type(instance), name)
        found = self._static.get(key)
        if found is None:
            try:
                inspect.getattr_static(instance, name)
            except AttributeError:
                found = False
            else:
                found = True
            self._static[key] = found
        return found

    def _is_async(self, func: Any) -> bool:
        """Whether a test function is a coroutine or an async generator
        function (once per function)."""
        target = getattr(func, "__func__", func)
        try:
            found = self._async.get(target)
        except TypeError:  # unhashable
            return _iscoroutinefunction(func) or _isasyncgenfunction(func)
        if found is None:
            found = self._async[target] = _iscoroutinefunction(func) or _isasyncgenfunction(func)
        return found

    def tags_changed(self) -> None:
        """A tag was added somewhere (to a case or a node, the engine's or
        the front-end's): every case's tags and xfail verdict are computed
        again when next read. A front-end whose items' marks can change
        while the engine runs calls it when they do."""
        self._tags_epoch += 1

    def _close_leftovers(self, *, stopped: bool) -> None:
        """Close what is still open (a run stopped by ``maxfail`` after a
        teardown error) but, with ``keep_session`` and no stop, the
        session's and the packages' lifetimes."""
        stack = self._setupstate.stack
        if not stack:
            return
        kept = [] if stopped or not self._keep else _kept(list(stack))
        self._close_scopes(kept, self.sink)

    def _close_scopes(self, needed: Sequence[tuple], sink: Sink | None) -> list[Failure]:
        """Close the open lifetimes ``needed`` doesn't share. With no case
        to blame, their errors go on a ``lifetime_finished`` of their own
        (into ``sink``, when there is one), the first one's failure."""
        if not self._setupstate.stack:
            return []
        began = perf_counter()
        try:
            errors = self._setupstate.teardown_exact(needed)
            failures = [self._failure(error, Phase.TEARDOWN) for error in errors]
        finally:
            self._raised_by.clear()
        if not failures:
            return []
        if sink is not None:
            sink.lifetime_started("session", "session")
            sink.lifetime_finished("session", failures[0], perf_counter() - began)
        return failures

    def _run_case(
        self,
        index: int,
        case: Case,
        following: Case | None,
        left: int,
        capture: bool,
        reproduce: Callable[[str], dict[str, Any] | None] | None,
    ) -> tuple[Result, tuple[str, int] | None]:
        sink = self.sink
        sink.test_started(case.id)
        result = Result(case.id)
        began = perf_counter()
        state: _CaseState | None = None
        stop: tuple[str, int] | None = None
        reasons: dict[Outcome, str | None] = {}
        context = self.services.get(Services.CONTEXT)
        cleanup = self.services.get(Services.CLEANUP)
        try:
            state = _CaseState(self, index, case)
            self._current = state
            self._items[id(case)] = state.item
            if context is not None:
                context.test = case.id
                context.lifetimes = []
            if cleanup is not None:
                cleanup.push()
            capturing = self._capture_start(result, capture)
            try:
                filtering = self._filters_start(state)
                try:
                    stop = self._phases(state, result, reasons, following, left, cleanup)
                finally:
                    self._filters_stop(filtering)
            finally:
                self._capture_stop(capturing, result)
        except BaseException as exc:  # noqa: BLE001 - the engine's own failure is the case's error
            result.add(Outcome.ERROR, self._failure(exc, Phase.SETUP))
            if isinstance(exc, KeyboardInterrupt):
                stop = ("KeyboardInterrupt", 2)
        finally:
            self._current = None
            self._set_env(None, None)
            result.duration = perf_counter() - began
            if result.outcome in _VERDICTS:
                reason = reasons.get(result.outcome)
                result.reason = reason or None
            if state is not None:
                result.properties = _properties(getattr(state.item, "user_properties", ()))
                if state.front_item is not None:
                    # pytest's, once every teardown hook ran: the values go.
                    front = state.front_item
                    aye_engine._guarded(lambda: setattr(front, "funcargs", None), None)
            if result.outcome is not None and result.outcome.is_bad:
                self._explain_bad(result, state, reproduce)
            self._raised_by.clear()
            sink.test_finished(result)
        return result, stop

    def _explain_bad(
        self,
        result: Result,
        state: _CaseState | None,
        reproduce: Callable[[str], dict[str, Any] | None] | None,
    ) -> None:
        """What a bad outcome carries beside its failures: the command that
        reruns it and the case's resources."""
        if reproduce is not None:
            result.reproduce = aye_engine._guarded(lambda: reproduce(result.id), None)
        if state is not None:
            result.resources = aye_engine._guarded(lambda: self._resources(state), [])

    def _phases(
        self,
        state: _CaseState,
        result: Result,
        reasons: dict[Outcome, str | None],
        following: Case | None,
        left: int,
        cleanup: Any,
    ) -> tuple[str, int] | None:
        """Setup, call (when setup passed) and teardown; why the run must
        stop, if it must."""
        stop = self._phase(state, Phase.SETUP, self._setup, result, reasons)
        if stop is None and state.setup_ok:
            called = perf_counter()
            stop = self._phase(state, Phase.TEST, self._call, result, reasons)
            result.body_duration = perf_counter() - called
            if state.call_ok and state.failed_subtests:
                count = state.failed_subtests
                message = f"contains {count} failed subtest{'s' if count != 1 else ''}"
                result.add(Outcome.FAILED, Failure(message, phase=Phase.TEST))
        last = stop is not None or (
            left > 0 and result.outcome is not None and result.outcome.is_bad and left <= 1
        )
        needed = self._needed(state, None if last else following, stopped=stop is not None)
        teardown_stop = self._phase(
            state,
            Phase.TEARDOWN,
            lambda s: self._teardown(s, needed, cleanup),
            result,
            reasons,
        )
        return stop or teardown_stop

    def _chain_keys(self, case: Case, index: int) -> list[tuple]:
        return [*self._node_keys(case.node), ("case", index)]

    def _needed(self, state: _CaseState, following: Case | None, *, stopped: bool) -> list[tuple]:
        """The lifetimes the case's teardown leaves open: the next case's,
        else (the run's last case) the session's and its packages' with
        ``keep_session``, else none."""
        if following is not None:
            return self._chain_keys(following, state.index + 1)
        if self._keep and not stopped:
            return _kept(state.keys)
        return []

    def _node_keys(self, node: Node) -> list[tuple]:
        """The keys of the lifetimes around ``node``'s cases, outermost
        first: the session's, then each node's of its chain (once per
        node)."""
        found = self._chains.get(id(node))
        if found is None or found[0] is not node:
            found = (node, [_SESSION, *(_node_key(n) for n in node.chain())])
            self._chains[id(node)] = found
        return found[1]

    def _phase(
        self,
        state: _CaseState,
        phase: Phase,
        run: Callable[[_CaseState], None],
        result: Result,
        reasons: dict[Outcome, str | None],
    ) -> tuple[str, int] | None:
        """Run one phase and fold what it raised into ``result``. Returns
        why the run must stop (an exit or an interrupt), if it must."""
        self._set_env(state.case, _WHEN[phase])
        raised = self._raised(state, phase, run, result)
        if phase is Phase.SETUP:
            state.setup_ok = raised is None
        elif phase is Phase.TEST:
            state.call_ok = raised is None
        if raised is not None:
            stopper = next(
                (
                    leaf
                    for leaf in _leaves(raised)
                    if isinstance(leaf, (KeyboardInterrupt, *self._exit_types))
                ),
                None,
            )
            if stopper is not None:
                result.add(Outcome.ERROR, self._failure(stopper, phase))
                if isinstance(stopper, KeyboardInterrupt):
                    return ("KeyboardInterrupt", 2)
                code = getattr(stopper, "returncode", None)
                status = code if isinstance(code, int) and not isinstance(code, bool) else 2
                return (aye_engine.exception_message(stopper), status)
        outcome, failures, reason = self._fold(state, phase, raised)
        if failures:
            for failure in failures:
                result.add(outcome, failure)
        else:
            result.add(outcome)
        if outcome in _VERDICTS:
            reasons[outcome] = reason
        return None

    def _raised(
        self,
        state: _CaseState,
        phase: Phase,
        run: Callable[[_CaseState], None],
        result: Result,
    ) -> BaseException | None:
        """Run a phase, its logging caught (the report's text into
        ``result``); what it raised."""
        log = self._log
        if log is None:
            try:
                run(state)
            except BaseException as exc:  # noqa: BLE001 - what the phase raised is its outcome
                return exc
            return None
        raised: BaseException | None = None
        catching = log.start(state, _WHEN[phase])
        try:
            run(state)
        except BaseException as exc:  # noqa: BLE001 - what the phase raised is its outcome
            raised = exc
        finally:
            text = log.stop(catching)
        if text:
            result.log = f"{result.log}\n{text}" if result.log else text
        return raised

    def _fold(  # noqa: PLR0911 - one return per verdict
        self, state: _CaseState, phase: Phase, raised: BaseException | None
    ) -> tuple[Outcome, list[Failure], str | None]:
        """One phase's outcome, as pytest reports it (its report outcome,
        then the xfail rewriting), in aye's words."""
        xfailed = None if self.runxfail else state.xfailed
        if raised is None:
            if phase is Phase.TEST and xfailed is not None:
                if xfailed.strict:
                    message = f"[XPASS(strict)] {xfailed.reason}"
                    return Outcome.FAILED, [Failure(message, phase=Phase.TEST)], None
                return Outcome.UNEXPECTED_PASS, [], xfailed.reason
            return Outcome.PASSED, [], None
        if not self.runxfail and isinstance(raised, self._xfail_types):
            return Outcome.EXPECTED_FAILURE, [], _outcome_message(raised)
        leaves = list(_leaves(raised))
        if all(self._is_skip(leaf) for leaf in leaves):
            return Outcome.SKIPPED, [], aye_engine.skip_reason(leaves[0]) or None
        if xfailed is not None and _raises_match(xfailed.raises, raised):
            return Outcome.EXPECTED_FAILURE, [], xfailed.reason
        outcome = Outcome.FAILED if phase is Phase.TEST else Outcome.ERROR
        shown = [leaf for leaf in leaves if not self._is_skip(leaf)] or leaves
        return outcome, [self._failure(leaf, phase) for leaf in shown], None

    def _is_skip(self, exc: BaseException) -> bool:
        if self._skip_types and isinstance(exc, self._skip_types):
            return True
        return aye_engine.is_skip(exc)

    def _fail(self, message: str) -> BaseException:
        """The failure the engine raises for what it finds wrong itself."""
        cls = self._fail_class
        try:
            return cls(message, pytrace=False)  # type: ignore[call-arg]
        except TypeError:
            return cls(message)

    # ------------------------------------------------------------ phases

    def _setup(self, state: _CaseState) -> None:
        case = state.case
        if state.filter_error is not None:
            raise state.filter_error
        skipped = self._evaluate_skip(state)
        if skipped is not None:
            raise aye_engine.Skip(skipped)
        if not self.runxfail:
            state.xfailed = self._evaluate_xfail(state)
            if state.xfailed is not None and not state.xfailed.run:
                raise XFailed("[NOTRUN] " + state.xfailed.reason)
        for hook in self.hooks.setup:
            hook(case)
        self._setupstate.setup(state.keys)
        state.instance = self._instance(state)
        for name in state.closure:
            if name not in state.funcargs:
                state.funcargs[name] = state.top.getfixturevalue(name)

    def _instance(self, state: _CaseState) -> Any:
        case = state.case
        if case.cls is None:
            return None
        if case.kind == "unittest":
            return case.cls(state.originalname)
        return case.cls()

    def _call(self, state: _CaseState) -> None:
        if not self.runxfail and state.xfailed is None:
            state.xfailed = self._evaluate_xfail(state)
            if state.xfailed is not None and not state.xfailed.run:
                raise XFailed("[NOTRUN] " + state.xfailed.reason)
        try:
            for hook in self.hooks.call:
                hook(state.case)
            if state.case.kind == "unittest":
                self._call_unittest(state)
            elif state.case.kind == "function":
                self._call_function(state)
            else:
                raise self._fail(
                    f"aye's engine has no runner for tests of kind {state.case.kind!r}"
                )
        finally:
            # The test may have added an xfail tag while it ran.
            if not self.runxfail and state.xfailed is None:
                state.xfailed = self._evaluate_xfail(state)

    def _call_function(self, state: _CaseState) -> None:
        case = state.case
        func = state.test_callable()
        if self._is_async(func):
            raise self._fail(_ASYNC_MESSAGE)
        kwargs = {name: self._argument(state, name) for name in case.argnames}
        returned = func(**kwargs)
        if hasattr(returned, "__await__") or hasattr(returned, "__aiter__"):
            close = getattr(returned, "close", None)
            if callable(close):
                aye_engine._guarded(close, None)
            raise self._fail(_ASYNC_MESSAGE)
        if returned is not None and self.return_warning is not None:
            warnings.warn(
                self.return_warning(
                    f"Test functions should return None, but {case.id} returned {type(returned)!r}.\n"
                    "Did you mean to use `assert` instead of `return`?\n"
                    "See https://docs.pytest.org/en/stable/how-to/assert.html#return-not-none "
                    "for more information."
                ),
                stacklevel=1,
            )

    def _argument(self, state: _CaseState, name: str) -> Any:
        if name in state.funcargs:
            return state.funcargs[name]
        if name in state.case.call_params:
            return state.case.call_params[name]
        value = state.top.getfixturevalue(name)
        state.funcargs[name] = value
        return value

    def _call_unittest(self, state: _CaseState) -> None:
        """``TestCase.run`` with a result recording what pytest's
        ``TestCaseFunction`` records: the first error is the call's, a second
        one the teardown's."""
        testcase = state.instance
        recorder = _UnittestResult(self, state)
        testcase(result=recorder)
        if state.unittest_errors:
            raise state.unittest_errors.pop(0)

    def _teardown(self, state: _CaseState, needed: list[tuple], cleanup: Any) -> None:
        errors: list[BaseException] = []
        try:
            for hook in self.hooks.teardown:
                hook(state.case)
        except BaseException as exc:  # noqa: BLE001 - the scopes still close
            errors.append(exc)
        if state.case.kind == "unittest" and state.unittest_errors:
            # pytest's next report of a TestCase takes its next recorded error.
            errors.insert(0, state.unittest_errors.pop(0))
        errors.extend(self._setupstate.teardown_exact(needed))
        if cleanup is not None:
            error = cleanup.pop()
            if error is not None:
                errors.append(error)
        _raise_all("errors during test teardown", errors)

    # ------------------------------------------------------------ conditions

    def _tags_named(self, state: _CaseState, name: str) -> list[Tag]:
        return [tag for tag in state.tags() if tag.name == name]

    def _evaluate_skip(self, state: _CaseState) -> str | None:
        """The skip reason when a ``skipif`` (closest first) or a ``skip``
        tag applies, else ``None``."""
        for name in self.policy.skip_tags:
            if name == "skip":
                continue
            for tag in self._tags_named(state, name):
                conditions = (tag.kwargs["condition"],) if "condition" in tag.kwargs else tag.args
                if not conditions:
                    return str(tag.kwargs.get("reason", ""))
                for condition in conditions:
                    holds, reason = self._condition(state, tag, condition)
                    if holds:
                        return reason
        if "skip" in self.policy.skip_tags:
            for tag in self._tags_named(state, "skip"):
                return _skip_reason_of(tag)
        return None

    def _evaluate_xfail(self, state: _CaseState) -> _Xfail | None:
        """The xfail verdict of the first ``xfail`` tag (closest first)
        whose condition holds, else ``None``: evaluated once, and again only
        when a tag may have been added since (``tags_version``)."""
        version = state.tags_version()
        if state._xfail_version != version:
            state._xfail = self._xfail_verdict(state)
            state._xfail_version = version
        return state._xfail

    def _xfail_verdict(self, state: _CaseState) -> _Xfail | None:
        for name in self.policy.xfail_tags:
            for tag in self._tags_named(state, name):
                run = tag.kwargs.get("run", True)
                strict = tag.kwargs.get("strict")
                if strict is None:
                    strict = self.policy.xfail_strict
                raises = tag.kwargs.get("raises")
                conditions = (tag.kwargs["condition"],) if "condition" in tag.kwargs else tag.args
                if not conditions:
                    return _Xfail(
                        str(tag.kwargs.get("reason", "")), bool(run), bool(strict), raises
                    )
                for condition in conditions:
                    holds, reason = self._condition(state, tag, condition)
                    if holds:
                        return _Xfail(reason, bool(run), bool(strict), raises)
        return None

    def _condition(self, state: _CaseState, tag: Tag, condition: Any) -> tuple[bool, str]:
        """Evaluate one ``skipif``/``xfail`` condition: a string in the
        policy's namespace and the test module's globals, anything else by
        ``bool``. Failing to evaluate it fails the phase, with pytest's
        words."""
        if isinstance(condition, str):
            namespace: dict[str, Any] = {
                "os": os,
                "sys": sys,
                "platform": platform,
                "config": self._config_of(state.case),
            }
            namespace.update(self.policy.condition_namespace)
            namespace.update(self._globals_of(state))
            try:
                code = compile(condition, f"<{tag.name} condition>", "eval")
                result = eval(code, namespace)
            except SyntaxError as exc:
                lines = [
                    f"Error evaluating {tag.name!r} condition",
                    "    " + condition,
                    "    " + " " * (exc.offset or 0) + "^",
                    "SyntaxError: invalid syntax",
                ]
                raise self._fail("\n".join(lines)) from None
            except Exception as exc:  # noqa: BLE001 - the user's condition
                lines = [
                    f"Error evaluating {tag.name!r} condition",
                    "    " + condition,
                    *_format_exception_only(type(exc), exc),
                ]
                raise self._fail("\n".join(lines)) from None
        else:
            try:
                result = bool(condition)
            except Exception as exc:  # noqa: BLE001
                lines = [
                    f"Error evaluating {tag.name!r} condition as a boolean",
                    *_format_exception_only(type(exc), exc),
                ]
                raise self._fail("\n".join(lines)) from None
        reason = tag.kwargs.get("reason")
        if reason is None:
            if isinstance(condition, str):
                reason = "condition: " + condition
            else:
                raise self._fail(
                    f"Error evaluating {tag.name!r}: you need to specify reason=STRING "
                    "when using booleans as conditions."
                )
        return bool(result), str(reason)

    def _globals_of(self, state: _CaseState) -> dict[str, Any]:
        func = state.case.func
        found = getattr(func, "__globals__", None)
        if found is None:
            found = getattr(inspect.unwrap(func), "__globals__", None) if callable(func) else None
        if found is None:
            found = getattr(state.module, "__dict__", None)
        return found or {}

    def _config_of(self, case: Case) -> Any:
        if self.config is not None:
            return self.config
        return case.extra.get("config")

    # ------------------------------------------------------------ resources

    def _closure(self, case: Case) -> list[str]:
        closure = list(case.closure)
        if case.kind != "unittest" or case.cls is None:
            return closure
        if any(name.startswith(_UNITTEST_CLASS_FIXTURES) for name in closure):
            return closure  # the front-end made it
        definition = self._unittest_class_def(case)
        # pytest registers it first among the class's autouse fixtures, so
        # it comes after the wider-scoped names and the class-scoped ones of
        # outer nodes.
        class_id = case.node.id
        position = len(closure)
        for at, name in enumerate(closure):
            found = self._lookup_chain(case, name)
            scope = found[0].scope if found else Scope.FUNCTION
            if scope is Scope.FUNCTION or (
                scope is Scope.CLASS and found and found[0].baseid.startswith(class_id)
            ):
                position = at
                break
        closure.insert(position, definition.name)
        return closure

    def _resources(self, state: _CaseState) -> list[tuple[str, str]]:
        """The case's resources and their scopes, for a bad outcome: those
        set up, in the order they were, then the rest of its closure (the
        one whose setup failed, those never reached), as far as they are
        defined; ``request`` and direct parameters left out."""
        case = state.case
        found: dict[str, str] = {}
        for name, slot in state.fixture_defs.items():
            found[name] = self._scope_of(case, name, slot.definition)
        for name in state.fixturenames:
            if name in found or name == "request" or name in case.call_params:
                continue
            chain = self._lookup_chain(case, name)
            if chain:
                found[name] = self._scope_of(case, name, chain[0])
        return list(found.items())

    @staticmethod
    def _scope_of(case: Case, name: str, definition: ResourceDef) -> str:
        """A resource's scope for ``case``: its parameter's when the case
        parametrizes it with one, else its definition's."""
        scope = definition.scope
        if name in case.resource_params:
            scope = case.param_scopes.get(name, scope)
        return scope.name.lower()

    def _unittest_class_def(self, case: Case) -> ResourceDef:
        cls = case.cls
        assert cls is not None
        definition = self._unittest_defs.get(cls)
        if definition is None:
            definition = ResourceDef(
                name=f"_unittest_setUpClass_fixture_{cls.__qualname__}",
                func=_unittest_class_fixture(cls),
                scope=Scope.CLASS,
                argnames=(),
                autouse=True,
                baseid=case.node.id,
                location=f"{case.location[0]}:{case.location[1]}",
            )
            self._unittest_defs[cls] = definition
        return definition

    def _lookup_chain(self, case: Case, name: str) -> list[ResourceDef]:
        """The definitions ``name`` can mean for ``case``, closest first:
        the node's, then the builtin behind them."""
        if case.kind == "unittest" and case.cls is not None:
            definition = self._unittest_defs.get(case.cls)
            if definition is not None and definition.name == name:
                return [definition]
        chain = list(case.node.visible.get(name, ()))
        builtin = self.builtins.get(name)
        if builtin is not None and builtin not in chain:
            chain.append(builtin)
        return chain

    def _slot(self, definition: ResourceDef) -> _Slot:
        slot = self._slots.get(definition)
        if slot is None:
            slot = self._slots[definition] = _Slot(definition)
        return slot

    def _value(self, request: Request, name: str) -> Any:
        found = self._active(request, name)
        if isinstance(found, _Slot):
            cached = found.cached
            if cached is None:
                raise self._fail(
                    f'The fixture value for "{name}" is not available. '
                    "This can happen when the fixture has already been torn down."
                )
            return cached[0]
        return found

    def _active(self, request: Request, name: str) -> Any:
        """What ``name`` means for ``request``: the request itself, a
        direct parameter's value, or the slot of the definition now live."""
        state = request._state
        case = state.case
        if name == "request":
            return request
        if name in case.call_params:
            return case.call_params[name]
        slot = state.fixture_defs.get(name)
        if slot is not None:
            self._check_scope(request, slot.definition)
            return slot
        chain = self._lookup_chain(case, name)
        depth = sum(1 for parent in request._requests() if parent.fixturename == name)
        if not chain or depth >= len(chain):
            available = self._available(state)
            message = self._lookup_message(name, bool(chain), available)
            error = FixtureLookupError(name, request, message)
            if not chain:
                error.available = available
            raise error
        definition = chain[depth]
        if name in case.resource_params:
            index, param, _ = case.resource_params[name]
            scope = case.param_scopes.get(name, definition.scope)
        else:
            index, param, scope = 0, _NOTSET, definition.scope
            if definition.params is not None:
                raise self._fail(
                    "The requested fixture has no parameter defined for test:\n"
                    f"    {case.id}\n\n"
                    f"Requested fixture '{name}' defined in:\n"
                    f"{definition.location or definition.baseid}"
                )
        self._check_scope(request, definition)
        sub = Request(
            self,
            state,
            fixturename=name,
            definition=definition,
            scope=scope,
            param=param,
            param_index=index,
            parent=request,
        )
        slot = self._slot(definition)
        self._execute(slot, sub)
        state.fixture_defs[name] = slot
        if name not in state.fixturenames:
            state.fixturenames.append(name)
        return slot

    def _available(self, state: _CaseState) -> list[str]:
        """The resource names visible to the case, sorted."""
        return sorted(
            {n for n, defs in state.case.node.visible.items() if defs} | set(self.builtins)
        )

    @staticmethod
    def _lookup_message(name: str, exists: bool, available: list[str]) -> str:
        if exists:
            message = f" recursive dependency involving fixture '{name}' detected"
        else:
            message = f"fixture '{name}' not found"
        message += "\n available fixtures: " + ", ".join(available)
        return message + "\n use 'pytest --fixtures [testpath]' for help on them."

    def _check_scope(self, request: Request, definition: ResourceDef) -> None:
        """A resource may use only resources of its own scope or wider
        (pytest's ``ScopeMismatch``)."""
        if request.fixturename is None or request._scope >= definition.scope:
            return
        stack = [r._definition for r in request._requests() if r._definition is not None][::-1]
        lines = "\n".join(_definition_line(d) for d in stack)
        raise self._fail(
            f"ScopeMismatch: You tried to access the {definition.scope.name.lower()} scoped "
            f"fixture {definition.name} with a {request.scope} scoped request object. "
            f"Requesting fixture stack:\n{lines}\n"
            f"Requested fixture:\n{_definition_line(definition)}"
        )

    def _execute(self, slot: _Slot, request: Request) -> Any:
        """Make ``slot``'s instance live for ``request``: its dependencies
        first, then the cached instance when the parameter matches, else a
        new one (the old one torn down now)."""
        dependencies = []
        for argname in slot.definition.argnames:
            found = self._active(request, argname)
            if isinstance(found, _Slot):
                dependencies.append(found)
        key = getattr(request, "param", None)
        if slot.cached is not None:
            if _same_key(key, slot.cached[1]):
                if slot.cached[2] is not None:
                    exc = slot.cached[2]
                    self._blame(exc, slot.definition.name)
                    raise exc.with_traceback(exc.__traceback__)
                return slot.cached[0]
            self._finish(slot)
        finish = functools.partial(self._finish, slot)
        for dependency in dependencies:
            dependency.finalizers.append(finish)
        try:
            return self._create(slot, request, key)
        finally:
            self._setupstate.addfinalizer(finish, request._key)

    def _create(self, slot: _Slot, request: Request, key: Any) -> Any:
        definition = slot.definition
        kwargs = {name: request.getfixturevalue(name) for name in definition.argnames}
        func = self._resolve_function(definition, request)
        if _iscoroutinefunction(func) or _isasyncgenfunction(func):
            auto = " with autouse=True" if definition.autouse else ""
            raise self._fail(
                f"{request._state.item.name!r} requested an async fixture {definition.name!r}{auto}, "
                "with no plugin or hook that handled it. This is an error, as pytest does not "
                "natively support it.\n"
                "See: https://docs.pytest.org/en/stable/deprecations.html#sync-test-depending-on-async-fixture"
            )
        try:
            if _isgeneratorfunction(func):
                generator = func(**kwargs)
                try:
                    value = next(generator)
                except StopIteration:
                    raise ValueError(f"{definition.name} did not yield a value") from None
                slot.finalizers.append(functools.partial(self._resume, generator, definition))
            else:
                value = func(**kwargs)
        except BaseException as exc:
            if self._cacheable(exc):
                slot.cached = (None, key, exc)
            self._blame(exc, definition.name)
            raise
        slot.cached = (value, key, None)
        return value

    def _resume(self, generator: Any, definition: ResourceDef) -> None:
        try:
            next(generator)
        except StopIteration:
            return
        raise self._fail(
            "fixture function has more than one 'yield':\n\n"
            f"{definition.location or definition.name}"
        )

    def _cacheable(self, exc: BaseException) -> bool:
        """Whether a resource's failure is cached for its scope: what a test
        can raise (pytest's ``TEST_OUTCOME``), not an interrupt."""
        if isinstance(exc, Exception) or self._is_skip(exc):
            return True
        return isinstance(exc, (*self._xfail_types, self._fail_class))

    def _finish(self, slot: _Slot) -> None:
        """Tear the slot's instance down: its finalizers, last in first out
        (its dependents' teardowns, its ``yield``'s rest, ``addfinalizer``
        functions), every one run whatever the others raise."""
        if slot.cached is None:
            return
        errors: list[BaseException] = []
        while slot.finalizers:
            fin = slot.finalizers.pop()
            try:
                fin()
            except BaseException as exc:  # noqa: BLE001 - every finalizer must run
                errors.append(exc)
                self._blame(exc, slot.definition.name)
        slot.cached = None
        slot.finalizers.clear()
        _raise_all(f'errors while tearing down fixture "{slot.definition.name}"', errors[::-1])

    def _resolve_function(self, definition: ResourceDef, request: Request) -> Callable[..., Any]:  # noqa: PLR0911 - one return per case
        """The callable to set ``definition`` up with: a method resource
        bound to the test's instance at function scope, else to an instance
        of its class made once, as pytest binds them."""
        func = definition.func
        if not definition.is_method:
            return func
        bound_to = getattr(func, "__self__", None)
        if isinstance(bound_to, type):
            return func  # a classmethod
        plain = getattr(func, "__func__", func)
        state = request._state
        instance = request.instance
        if instance is not None:
            owner = type(bound_to) if bound_to is not None else None
            if owner is not None and not isinstance(instance, owner):
                return func
            return plain.__get__(instance, type(instance))
        if bound_to is not None:
            return func
        cls = state.case.cls
        if cls is None:
            return func
        holder = self._class_instances.get(cls)
        if holder is None:
            # pytest's holder: a TestCase takes a method name, and "runTest"
            # is the one it accepts without such a method.
            holder = cls("runTest") if state.case.kind == "unittest" else cls()
            self._class_instances[cls] = holder
        return plain.__get__(holder, cls)

    def _scope_key(self, state: _CaseState, scope: Scope, definition: ResourceDef | None) -> tuple:  # noqa: PLR0911 - one return per case
        """The scope an instance lives in for ``state``'s case: the case, its
        class (the case when it has none), its module, the package that
        defines the resource, or the session."""
        if scope is Scope.FUNCTION:
            return state.key
        chain = state.case.node.chain()
        if scope is Scope.CLASS:
            for node in reversed(chain):
                if node.kind == "class":
                    return _node_key(node)
            return state.key
        if scope is Scope.MODULE:
            for node in reversed(chain):
                if node.kind == "module":
                    return _node_key(node)
            return _SESSION
        if scope is Scope.PACKAGE and definition is not None:
            base = _directory(definition.baseid)
            for node in reversed(chain):
                if node.kind == "package" and _directory(node.id) == base:
                    return _node_key(node)
        return _SESSION

    def _view_of_key(self, state: _CaseState, key: tuple) -> Any:
        if key == state.key:
            return state.item
        if key == _SESSION:
            return self._session_view()
        for node in state.case.node.chain():
            if _node_key(node) == key:
                return self._view_of_node(node)
        return state.item

    def _session_view(self) -> NodeView:
        view = self._views.get(_SESSION)
        if view is None:
            view = self._views[_SESSION] = NodeView(self, _SESSION, None, None)
        return view

    def _view_of_node(self, node: Node) -> NodeView:
        key = _node_key(node)
        view = self._views.get(key)
        if view is None:
            parent = (
                self._view_of_node(node.parent) if node.parent is not None else self._session_view()
            )
            view = self._views[key] = NodeView(self, key, node, parent)
        return view

    # ------------------------------------------------------------ capture, logs, warnings

    def _capture_start(self, result: Result, capture: bool) -> Any:
        """Capture the case's output into ``result`` (stdin closed to
        reads) while capture is on: the capture service's, else the
        session's pair of streams. Returns what ``_capture_stop`` undoes."""
        self._outer_streams = (sys.stdout, sys.stderr)
        if not capture:
            return None
        saved_in = sys.stdin
        sys.stdin = _NoInput()
        service = self.services.get(Services.CAPTURE)
        if service is not None:
            manager = service.capture(result)
            try:
                manager.__enter__()
            except BaseException:
                sys.stdin = saved_in
                raise
            return saved_in, manager, None, None
        streams = self._streams or (aye_engine.CaptureIO(), aye_engine.CaptureIO())
        self._streams = None
        saved = sys.stdout, sys.stderr
        sys.stdout, sys.stderr = streams
        return saved_in, None, streams, saved

    def _capture_stop(self, capturing: Any, result: Result) -> None:
        """Undo ``_capture_start``: the output into ``result``, the streams
        kept for the next case when it can write into them."""
        try:
            if capturing is None:
                return
            saved_in, manager, streams, saved = capturing
            try:
                if manager is not None:
                    manager.__exit__(None, None, None)
                    return
                sys.stdout, sys.stderr = saved
                result.stdout += _taken(streams[0])
                result.stderr += _taken(streams[1])
                if _reusable(streams[0]) and _reusable(streams[1]):
                    self._streams = streams
            finally:
                sys.stdin = saved_in
        finally:
            self._outer_streams = None

    def _filters_start(self, state: _CaseState) -> Any:
        """The case's warnings filters, for its three phases: pytest's
        defaults, the ini lines, ``-W``, then each ``filterwarnings`` tag,
        closest first, so the outermost wins. Returns what
        ``_filters_stop`` undoes."""
        if "warnings" in self.disabled:
            return None
        catching = warnings.catch_warnings(record=True)
        catching.__enter__()
        try:
            filterwarnings = warnings.filterwarnings
            if not sys.warnoptions:
                filterwarnings("always", category=DeprecationWarning)
                filterwarnings("always", category=PendingDeprecationWarning)
            parsed, error = self._configured_filters(self._config_of(state.case))
            for args in parsed:
                filterwarnings(*args)
            if error is not None:
                state.filter_error = error
            else:
                try:
                    for tag in state.tags():
                        if tag.name == "filterwarnings":
                            for arg in tag.args:
                                filterwarnings(*self._tag_filter(arg))
                except Exception as exc:  # noqa: BLE001 - a bad filter fails the case's setup
                    state.filter_error = exc
        except BaseException:
            catching.__exit__(None, None, None)
            raise
        return catching

    @staticmethod
    def _filters_stop(catching: Any) -> None:
        if catching is not None:
            catching.__exit__(None, None, None)

    def _configured_filters(
        self, config: Any
    ) -> tuple[list[tuple[Any, ...]], BaseException | None]:
        """The configuration's filters (the ini lines, then ``-W``), parsed
        once per run, and what the first bad one raised (the ones before it
        still apply)."""
        found = self._config_filters.get(id(config))
        if found is None:
            parsed: list[tuple[Any, ...]] = []
            error: BaseException | None = None
            try:
                for line in _lines(_ini(config, "filterwarnings")):
                    parsed.append(parse_warning_filter(line, escape=False))
                for line in _lines(_option(config, "pythonwarnings")):
                    parsed.append(parse_warning_filter(line, escape=True))
            except Exception as exc:  # noqa: BLE001 - a bad filter fails each case's setup
                error = exc
            found = self._config_filters[id(config)] = (parsed, error)
        return found

    def _tag_filter(self, arg: Any) -> tuple[Any, ...]:
        """A ``filterwarnings`` tag's line, parsed (once per line)."""
        if not isinstance(arg, str):
            return parse_warning_filter(arg, escape=False)
        found = self._tag_filters.get(arg)
        if found is None:
            found = self._tag_filters[arg] = parse_warning_filter(arg, escape=False)
        return found

    def _set_env(self, case: Case | None, when: str | None) -> None:
        name = self.current_test_env
        if not name:
            return
        if case is None:
            _environ.pop(name, None)
        else:
            _environ[name] = f"{case.id} ({when})".replace("\x00", "(null)")

    @contextmanager
    def _uncaptured(self) -> Iterator[None]:
        """Output to where it went before the case's capture (and a capture
        resource's) took it, while the block runs: ``capsys.disabled()``."""
        fixture = self._capture_fixture
        if fixture is not None:
            fixture._suspend()
        saved = (sys.stdout, sys.stderr)
        outer = self._outer_streams
        if outer is not None:
            sys.stdout, sys.stderr = outer
        try:
            yield
        finally:
            sys.stdout, sys.stderr = saved
            if fixture is not None:
                fixture._resume()

    # ------------------------------------------------------------ failures

    def _failure(self, exc: BaseException, phase: Phase) -> Failure:
        """``exc`` as aye's structured failure, with the user's frames only
        (``aye_engine.describe_frames``) and, for a failed comparison, its
        operands, explanation and structured diff; for a setup or teardown,
        the resource that raised it, and for an unknown resource, the
        names closest to it."""
        frames = aye_engine._guarded(lambda: self._frames(exc), [])
        location = (frames[-1]["path"], frames[-1]["line"]) if frames else None
        assertion = (
            aye_engine._guarded(lambda: self._assertion(exc), None)
            if isinstance(exc, AssertionError)
            else None
        )
        resource = self._blamed(exc) if phase is not Phase.TEST else None
        return Failure(
            aye_engine._guarded(lambda: _crash_message(exc), type(exc).__name__),
            phase=phase,
            exception=type(exc).__name__,
            assertion=assertion,
            location=location,
            frames=frames,
            detail=aye_engine._guarded(lambda: self._detail(exc), None),
            resource=resource,
            hint=aye_engine._guarded(lambda: _lookup_hint(exc), None),
        )

    def _blame(self, exc: BaseException, name: str) -> None:
        """Remember that the resource ``name``'s setup or teardown raised
        ``exc``, unless one it called raised it first (that one is
        blamed), until the case's result is built."""
        if self._blamed(exc) is None:
            self._raised_by.append((exc, name))

    def _blamed(self, exc: BaseException) -> str | None:
        """The resource whose setup or teardown raised ``exc``, if one
        did."""
        for raised, name in self._raised_by:
            if raised is exc:
                return name
        return None

    def _hidden(self, frame: Any) -> bool:
        filename = frame.f_code.co_filename
        if self.paths.is_internal(filename):
            return True
        if os.path.normcase(os.path.abspath(filename)) in _OWN_FILES:
            return True
        if frame.f_globals.get("__unittest"):
            return True
        hide = frame.f_locals.get("__tracebackhide__", frame.f_globals.get("__tracebackhide__"))
        return bool(hide) and not callable(hide)

    def _frames(self, exc: BaseException) -> list[dict[str, Any]]:
        frames = []
        entries = []
        tb = exc.__traceback__
        while tb is not None:
            frame = tb.tb_frame
            if not self._hidden(frame):
                filename = frame.f_code.co_filename
                line = tb.tb_lineno or 0
                entry: dict[str, Any] = {
                    "path": self.paths.relative(filename),
                    "line": line,
                    "function": frame.f_code.co_name,
                }
                source = _getline(filename, line).strip()
                if source:
                    entry["source"] = source
                frames.append(entry)
                entries.append(tb)
            tb = tb.tb_next
        aye_engine.describe_frames(frames, entries, self.paths, exc)
        return frames

    def _detail(self, exc: BaseException) -> str:
        tb = exc.__traceback__
        while tb is not None and self._hidden(tb.tb_frame):
            tb = tb.tb_next
        return "".join(_format_exception(type(exc), exc, tb))

    def _assertion(self, exc: BaseException) -> dict[str, Any] | None:
        """A rewritten assert's comparison (``exc._aye_assert``): its
        operands, the explanation, which side was expected, each side's
        source and, for ``==``, how the values differ
        (``aye_engine.compare``)."""
        data = getattr(exc, "_aye_assert", None)
        if not isinstance(data, dict) or "op" not in data:
            return None
        op = str(data["op"])
        left, right = data.get("left"), data.get("right")
        assertion = {
            "op": op,
            "left": saferepr(left, maxsize=2000),
            "right": saferepr(right, maxsize=2000),
            "diff": explain(op, left, right, self.verbose),
        }
        expected = data.get("expected")
        if expected in ("left", "right"):
            assertion["expected"] = expected
        for key, source in (("left_source", "left_src"), ("right_source", "right_src")):
            if isinstance(data.get(source), str):
                assertion[key] = data[source]
        if op == "==":
            compared = aye_engine.compare(left, right, assertion.get("expected"))
            if compared is not None:
                assertion["compare"] = compared
        return assertion


def _taken(stream: Any) -> str:
    """What a case wrote to a capture stream (``aye_engine.CaptureIO``),
    which is emptied for the next one."""
    try:
        if stream.buffer.tell() == 0 and getattr(stream, "_file", None) is None:
            return ""  # nothing written
    except (AttributeError, OSError, ValueError):
        pass  # closed, or replaced: read what it kept
    value = stream.release()
    if value and not stream.closed:
        try:
            stream.seek(0)
            stream.truncate()
        except (OSError, ValueError):
            stream.close()
    return value


def _reusable(stream: Any) -> bool:
    """Whether the next case can write into this capture stream: still
    open, empty, with no file descriptor given out, as made."""
    try:
        return (
            not stream.closed
            and getattr(stream, "_file", None) is None
            and stream.buffer.tell() == 0
            and stream.errors == "replace"
            and stream.encoding == "utf-8"
            and not stream.line_buffering
        )
    except (AttributeError, OSError, ValueError):
        return False


def _crash_message(exc: BaseException) -> str:
    """One line for a failure, as pytest's crash line reads: an assertion's
    ``assert …`` (or ``AssertionError: msg``), a lookup error's own text,
    else ``Type: first line``."""
    if isinstance(exc, FixtureLookupError):
        return (str(exc).strip().splitlines() or ["FixtureLookupError"])[0]
    text = str(exc).strip()
    first = text.splitlines()[0] if text else ""
    if isinstance(exc, AssertionError):
        if not first:
            return type(exc).__name__
        return first if first.startswith("assert ") else f"{type(exc).__name__}: {first}"
    return f"{type(exc).__name__}: {first}" if first else type(exc).__name__


def _session_stop(session: Any) -> tuple[str, int]:
    """Why a test's ``session.shouldstop`` (status 2) or ``shouldfail``
    (status 1) stops the run."""
    for flag, status in (("shouldstop", 2), ("shouldfail", 1)):
        value = getattr(session, flag, False)
        if value:
            return (value if isinstance(value, str) else f"session.{flag}", status)
    return ("session.shouldstop", 2)


def _outcome_message(exc: BaseException) -> str:
    msg = getattr(exc, "msg", None)
    if msg is None:
        msg = getattr(exc, "reason", None)
    if msg is None:
        msg = str(exc)
    return str(msg)


def _raises_match(raises: Any, exc: BaseException) -> bool:
    if raises is None:
        return True
    try:
        if isinstance(raises, type | tuple):
            return isinstance(exc, raises)
        matches = getattr(raises, "matches", None)
        if callable(matches):
            return bool(matches(exc))
    except Exception:  # noqa: BLE001 - a broken ``raises`` matches nothing
        return False
    return False


def _same_key(new: Any, old: Any) -> bool:
    try:
        return bool(new == old)
    except (ValueError, RuntimeError):
        return new is old


def _skip_reason_of(tag: Tag) -> str:
    """A ``skip`` tag's reason: its first argument or ``reason``,
    ``unconditional skip`` without one."""
    unknown = set(tag.kwargs) - {"reason"}
    if unknown or len(tag.args) > 1 or (tag.args and "reason" in tag.kwargs):
        raise TypeError(
            "Skip() got an unexpected keyword argument - maybe you meant pytest.mark.skipif?"
        )
    if tag.args:
        return str(tag.args[0])
    return str(tag.kwargs.get("reason", "unconditional skip"))


def _directory(nodeid: str) -> str:
    text = nodeid.removesuffix("/__init__.py").rstrip("/")
    return "" if text == "." else text


def _definition_line(definition: ResourceDef) -> str:
    try:
        signature = str(inspect.signature(definition.func))
    except (TypeError, ValueError):
        signature = "(...)"
    where = definition.location or definition.baseid or "<unknown>"
    return f"{where}:  def {definition.name}{signature}"


def _properties(pairs: Iterable[Any]) -> list[tuple[str, str]]:
    properties = []
    for pair in pairs or ():
        text = aye_engine._guarded(lambda pair=pair: _property_text(pair), None)
        if text is not None:  # a broken pair loses one property
            properties.append(text)
    return properties


def _property_text(pair: Any) -> tuple[str, str]:
    name, value = pair
    return str(name), str(value)


class _NoInput:
    """``sys.stdin`` while output is captured: reading fails, as pytest's
    capture makes it fail, rather than blocking or reading the worker's."""

    encoding = "utf-8"
    closed = False
    buffer = property(lambda self: self)

    def _refuse(self, *args: Any) -> Any:
        raise OSError("reading from stdin while output is captured!  Consider using `-s`.")

    read = readline = readlines = __next__ = _refuse

    def __iter__(self) -> _NoInput:
        return self

    def fileno(self) -> int:
        raise io.UnsupportedOperation("redirected stdin is pseudofile, has no fileno()")

    def isatty(self) -> bool:
        return False

    def close(self) -> None:
        pass

    def readable(self) -> bool:
        return False


# ------------------------------------------------------------------ unittest


# The class-scoped resources a front-end may make for a TestCase class
# (pytest's names), in which case the engine makes none.
_UNITTEST_CLASS_FIXTURES = ("_unittest_setUpClass_fixture_", "_unittest_skip_fixture_")


def _item_like(item: Any) -> Any:
    """``item`` when it can stand for ``request.node`` (it reads and adds
    marks), else ``None``."""
    if item is None:
        return None
    if callable(getattr(item, "iter_markers", None)) and callable(
        getattr(item, "add_marker", None)
    ):
        return item
    return None


def _unittest_class_fixture(cls: type) -> Callable[[], Iterator[None]]:
    """``setUpClass`` and ``tearDownClass`` (with ``doClassCleanups``) as a
    class-scoped resource, as pytest registers them; a class unittest skips
    skips every test in it."""

    def raise_class_cleanup_errors() -> None:
        errors = [exc for _, exc, _ in getattr(cls, "tearDown_exceptions", None) or ()]
        _raise_all("Unittest class cleanup errors", errors)

    def unittest_class() -> Iterator[None]:
        if getattr(cls, "__unittest_skip__", False):
            raise unittest.SkipTest(getattr(cls, "__unittest_skip_why__", ""))
        setup = getattr(cls, "setUpClass", None)
        teardown = getattr(cls, "tearDownClass", None)
        cleanup = getattr(cls, "doClassCleanups", lambda: None)
        if setup is not None:
            try:
                setup()
            except Exception:
                cleanup()
                raise_class_cleanup_errors()
                raise
        yield
        try:
            if teardown is not None:
                teardown()
        finally:
            cleanup()
            raise_class_cleanup_errors()

    return unittest_class


class _UnittestResult(unittest.TestResult):
    """What ``TestCase.run`` reports to, recording as pytest's
    ``TestCaseFunction`` does: errors and failures in order, a skip as a skip,
    an expected failure as an xfail, an unexpected success as a failure, and
    each subtest as a ``sub_outcome``."""

    def __init__(self, engine: Engine, state: _CaseState) -> None:
        super().__init__()
        self._engine = engine
        self._state = state

    def _record(self, exc: BaseException) -> None:
        self._state.unittest_errors.append(exc)

    def startTest(self, test: Any) -> None:
        pass

    def stopTest(self, test: Any) -> None:
        pass

    def addSuccess(self, test: Any) -> None:
        pass

    def addDuration(self, test: Any, elapsed: float) -> None:
        pass

    def addError(self, test: Any, err: Any) -> None:
        exc = err[1]
        if isinstance(exc, self._engine._exit_types):
            raise exc
        self._record(exc)

    def addFailure(self, test: Any, err: Any) -> None:
        self._record(err[1])

    def addSkip(self, test: Any, reason: str) -> None:
        if isinstance(test, _SUBTEST):
            self._sub(test, Outcome.SKIPPED, None, reason)
            return
        self._record(aye_engine.Skip(str(reason)))

    def addExpectedFailure(self, test: Any, err: Any) -> None:
        self._record(XFailed(""))

    def addUnexpectedSuccess(self, test: Any) -> None:
        self._record(self._engine._fail("Unexpected success"))

    def addSubTest(self, test: Any, subtest: Any, err: Any) -> None:
        if err is None:
            self._sub(subtest, Outcome.PASSED, None, None)
            return
        exc = err[1]
        if self._engine._is_skip(exc):
            self._sub(subtest, Outcome.SKIPPED, None, aye_engine.skip_reason(exc))
            return
        self._state.failed_subtests += 1
        self._sub(subtest, Outcome.FAILED, self._engine._failure(exc, Phase.TEST), None)

    def _sub(
        self, subtest: Any, outcome: Outcome, failure: Failure | None, reason: str | None
    ) -> None:
        message = getattr(subtest, "_message", None)
        if message is _SUBTEST_SENTINEL:
            message = None
        params = dict(getattr(subtest, "params", {}) or {})
        self._engine.sink.sub_outcome(
            self._state.case.id, _subtest_name(message, params), outcome, failure, reason=reason
        )


_SUBTEST = getattr(unittest.case, "_SubTest", ())
_SUBTEST_SENTINEL = getattr(unittest.case, "_subtest_msg_sentinel", object())


def _subtest_name(message: Any, params: Mapping[str, Any]) -> str:
    parts = [] if message is None else [str(message)]
    if params:
        values = (
            f"{key}={value if isinstance(value, str) else saferepr(value)}"
            for key, value in params.items()
        )
        parts.append(f"({', '.join(values)})")
    return " ".join(parts) or "<subtest>"


# ------------------------------------------------------------------ warnings filters


def parse_warning_filter(arg: str, *, escape: bool) -> tuple[str, str, type[Warning], str, int]:
    """``action:message:category:module:lineno`` as ``warnings.filterwarnings``
    arguments (pytest's syntax: the message and module are regexes, escaped
    for ``-W``)."""
    parts = arg.split(":")
    if len(parts) > _FILTER_FIELDS:
        raise ValueError(f"too many fields (max 5) in warning filter: {arg!r}")
    parts += [""] * (_FILTER_FIELDS - len(parts))
    action_text, message, category_text, module, lineno_text = (part.strip() for part in parts)
    action = _warning_action(action_text)
    category = _warning_category(category_text)
    if message and escape:
        message = re.escape(message)
    if module and escape:
        module = re.escape(module) + r"\Z"
    if lineno_text:
        lineno = int(lineno_text)
        if lineno < 0:
            raise ValueError("number is negative")
    else:
        lineno = 0
    re.compile(message)
    re.compile(module)
    return action, message, category, module, lineno


_FILTER_FIELDS = 5
_ACTIONS = ("default", "always", "all", "ignore", "module", "once", "error")


def _warning_action(text: str) -> str:
    if not text:
        return "default"
    for action in _ACTIONS:
        if action.startswith(text):
            return action
    raise ValueError(f"invalid action: {text!r}")


def _warning_category(text: str) -> type[Warning]:
    if not text:
        return Warning
    if "." in text:
        module_name, _, name = text.rpartition(".")
        module = sys.modules.get(module_name)
        if module is None:
            module = __import__(module_name, None, None, [name])
        category = getattr(module, name)
    else:
        category = getattr(builtins, text)
    if not (isinstance(category, type) and issubclass(category, Warning)):
        raise TypeError(f"{category!r} is not a Warning subclass")
    return category


# ------------------------------------------------------------------ logging


class LogCaptureHandler(logging.StreamHandler):
    """A handler keeping its records and their text; a record it can't
    format fails the test (``handleError`` re-raises), as under pytest."""

    def __init__(self) -> None:
        super().__init__(io.StringIO())
        self.records: list[logging.LogRecord] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.records.append(record)
        super().emit(record)

    def reset(self) -> None:
        self.records = []
        self.stream = io.StringIO()

    def clear(self) -> None:
        self.records.clear()
        self.stream = io.StringIO()

    def handleError(self, record: logging.LogRecord) -> None:
        if logging.raiseExceptions:
            raise  # noqa: PLE0704 - the error being handled


class _LogCapture:
    """Per phase, a handler for ``caplog`` and one for the report, on the
    root logger and on every logger that doesn't propagate."""

    def __init__(self, config: Any) -> None:
        self.level = _log_level(_option(config, "log_level") or _ini(config, "log_level"))
        fmt = _option(config, "log_format") or _ini(config, "log_format") or DEFAULT_LOG_FORMAT
        datefmt = _option(config, "log_date_format") or _ini(config, "log_date_format")
        self.formatter = logging.Formatter(str(fmt), datefmt)
        self.caplog_handler = LogCaptureHandler()
        self.caplog_handler.setFormatter(self.formatter)
        self.report_handler = LogCaptureHandler()
        self.report_handler.setFormatter(self.formatter)

    def start(self, state: _CaseState, when: str) -> tuple[list[logging.Logger], int]:
        """Catch a phase's records: both handlers, emptied, on the root
        logger and on every logger that doesn't propagate (one pass over
        the loggers, as pytest's nested ``catching_logs`` attach them,
        ``caplog``'s first), the root's level lowered to the configured
        one. Returns what ``stop`` undoes."""
        handlers = (self.caplog_handler, self.report_handler)
        level = self.level
        root = logging.getLogger()
        if level is not None:
            for handler in handlers:
                handler.setLevel(level)
        attached = [root]
        attached += [
            logger
            for logger in list(root.manager.loggerDict.values())
            if isinstance(logger, logging.Logger) and not logger.propagate and logger is not root
        ]
        for logger in attached:
            for handler in handlers:
                logger.addHandler(handler)
        original = root.level
        if level is not None:
            root.setLevel(min(original, level))
        self.caplog_handler.reset()
        self.report_handler.reset()
        state.log_records[when] = self.caplog_handler.records
        return attached, original

    def stop(self, catching: tuple[list[logging.Logger], int]) -> str:
        """Undo ``start``; the phase's report text."""
        attached, original = catching
        if self.level is not None:
            logging.getLogger().setLevel(original)
        for logger in attached:
            logger.removeHandler(self.report_handler)
            logger.removeHandler(self.caplog_handler)
        return self.report_handler.stream.getvalue().strip()


_ANSI = re.compile(r"\x1b\[[\d;]+m")


class LogCaptureFixture:
    """``caplog``: the records and text logged during the current phase,
    and levels set for the test, restored at its teardown."""

    def __init__(self, item: Any, *, _ispytest: bool = False) -> None:
        self._item = item
        self._state: _CaseState = item._state
        self._log: _LogCapture = item._engine._log
        self._initial_handler_level: int | None = None
        self._initial_logger_levels: dict[str | None, int] = {}
        self._initial_disabled_level: int | None = None

    def _finalize(self) -> None:
        if self._initial_handler_level is not None:
            self.handler.setLevel(self._initial_handler_level)
        for name, level in self._initial_logger_levels.items():
            logging.getLogger(name).setLevel(level)
        if self._initial_disabled_level is not None:
            logging.disable(self._initial_disabled_level)
            self._initial_disabled_level = None

    @property
    def handler(self) -> LogCaptureHandler:
        return self._log.caplog_handler

    def get_records(self, when: str) -> list[logging.LogRecord]:
        return self._state.log_records.get(when, [])

    @property
    def text(self) -> str:
        return _ANSI.sub("", self.handler.stream.getvalue())

    @property
    def records(self) -> list[logging.LogRecord]:
        return self.handler.records

    @property
    def record_tuples(self) -> list[tuple[str, int, str]]:
        return [(r.name, r.levelno, r.getMessage()) for r in self.records]

    @property
    def messages(self) -> list[str]:
        return [r.getMessage() for r in self.records]

    def clear(self) -> None:
        self.handler.clear()

    def _force_enable(self, level: int | str, logger: logging.Logger) -> int:
        original = logger.manager.disable
        if isinstance(level, str):
            level = logging.getLevelName(level)
        if not isinstance(level, int):
            logging.disable(logging.NOTSET)
        elif not logger.isEnabledFor(level):
            logging.disable(max(level - 10, logging.NOTSET))
        return original

    def set_level(self, level: int | str, logger: str | None = None) -> None:
        target = logging.getLogger(logger)
        self._initial_logger_levels.setdefault(logger, target.level)
        target.setLevel(level)
        if self._initial_handler_level is None:
            self._initial_handler_level = self.handler.level
        self.handler.setLevel(level)
        disabled = self._force_enable(level, target)
        if self._initial_disabled_level is None:
            self._initial_disabled_level = disabled

    @contextmanager
    def at_level(self, level: int | str, logger: str | None = None) -> Iterator[None]:
        target = logging.getLogger(logger)
        original = target.level
        target.setLevel(level)
        handler_level = self.handler.level
        self.handler.setLevel(level)
        disabled = self._force_enable(level, target)
        try:
            yield
        finally:
            target.setLevel(original)
            self.handler.setLevel(handler_level)
            logging.disable(disabled)

    @contextmanager
    def filtering(self, filter_: logging.Filter) -> Iterator[None]:
        self.handler.addFilter(filter_)
        try:
            yield
        finally:
            self.handler.removeFilter(filter_)


# ------------------------------------------------------------------ capture resources

CaptureResult = namedtuple("CaptureResult", ["out", "err"])


class _SysCapture:
    """``sys.stdout`` or ``sys.stderr`` replaced by a capturing stream."""

    def __init__(self, name: str) -> None:
        self.name = name
        self.stream = aye_engine.CaptureIO()
        self.old: Any = None

    def start(self) -> None:
        self.old = getattr(sys, self.name)
        setattr(sys, self.name, self.stream)

    def suspend(self) -> None:
        setattr(sys, self.name, self.old)

    def resume(self) -> None:
        setattr(sys, self.name, self.stream)

    def snap(self) -> bytes:
        stream = self.stream
        aye_engine._guarded(stream.flush, None)
        data = aye_engine._guarded(lambda: bytes(stream.buffer.value()), b"")
        extra = getattr(stream, "_file", None)
        if extra is not None:
            data += aye_engine._guarded(lambda: _read_and_empty(extra), b"")
        if not stream.closed:
            stream.seek(0)
            stream.truncate()
        return data

    def done(self) -> bytes:
        data = self.snap()
        setattr(sys, self.name, self.old)
        aye_engine._guarded(self.stream.release, None)
        return data


def _read_and_empty(file: Any) -> bytes:
    file.seek(0)
    data = bytes(file.read())
    file.seek(0)
    file.truncate()
    return data


class _FDCapture:
    """A file descriptor (1 or 2) pointed at a temporary file, and the
    matching ``sys`` stream writing to it, so that both Python's writes and
    the process's (``os.write``, a subprocess) are captured."""

    def __init__(self, fd: int) -> None:
        self.fd = fd
        self.name = "stdout" if fd == 1 else "stderr"
        self.file = tempfile.TemporaryFile(buffering=0)  # noqa: SIM115 - closed by ``done``
        self.saved: int | None = None
        self.old: Any = None
        self.stream: Any = None

    def start(self) -> None:
        self._flush()
        self.saved = _dup(self.fd)
        _dup2(self.file.fileno(), self.fd)
        raw = io.FileIO(_dup(self.file.fileno()), "wb", closefd=True)
        self.stream = io.TextIOWrapper(
            raw, encoding="utf-8", errors="replace", newline="", write_through=True
        )
        self.old = getattr(sys, self.name)
        setattr(sys, self.name, self.stream)

    def _flush(self) -> None:
        stream = getattr(sys, self.name, None)
        if stream is not None:
            aye_engine._guarded(stream.flush, None)

    def suspend(self) -> None:
        self._flush()
        if self.saved is not None:
            _dup2(self.saved, self.fd)
        setattr(sys, self.name, self.old)

    def resume(self) -> None:
        _dup2(self.file.fileno(), self.fd)
        setattr(sys, self.name, self.stream)

    def snap(self) -> bytes:
        if self.stream is not None:
            aye_engine._guarded(self.stream.flush, None)
        return _read_and_empty(self.file)

    def done(self) -> bytes:
        data = self.snap()
        self.suspend()
        if self.saved is not None:
            _close(self.saved)
            self.saved = None
        aye_engine._guarded(self.stream.close, None)
        aye_engine._guarded(self.file.close, None)
        return data


class CaptureFixture:
    """``capsys``, ``capfd``, ``capsysbinary``, ``capfdbinary``: the test's
    output since the last ``readouterr()``, as text or bytes."""

    def __init__(self, kind: str, engine: Engine, *, _ispytest: bool = False) -> None:
        self._kind = kind
        self._engine = engine
        self._binary = kind.endswith("binary")
        self._captures: tuple[Any, Any] | None = None
        empty: Any = b"" if self._binary else ""
        self._captured_out = empty
        self._captured_err = empty

    def _start(self) -> None:
        if self._kind.startswith("fd"):
            out, err = _FDCapture(1), _FDCapture(2)
        else:
            out, err = _SysCapture("stdout"), _SysCapture("stderr")
        out.start()
        try:
            err.start()
        except BaseException:
            out.done()
            raise
        self._captures = (out, err)

    def _decode(self, data: bytes) -> Any:
        return data if self._binary else data.decode("utf-8", "replace")

    def close(self) -> None:
        if self._captures is None:
            return
        out, err = self._captures
        self._captures = None
        left_err = self._decode(err.done())
        left_out = self._decode(out.done())
        self._captured_out += left_out
        self._captured_err += left_err
        # What wasn't read goes where the output went before (the test's
        # capture), so the report shows it.
        for stream, text in ((sys.stdout, left_out), (sys.stderr, left_err)):
            if text:
                aye_engine._guarded(lambda s=stream, t=text: _write(s, t), None)

    def readouterr(self) -> CaptureResult:
        out, err = self._captured_out, self._captured_err
        if self._captures is not None:
            out += self._decode(self._captures[0].snap())
            err += self._decode(self._captures[1].snap())
        empty: Any = b"" if self._binary else ""
        self._captured_out = self._captured_err = empty
        return CaptureResult(out, err)

    def _suspend(self) -> None:
        if self._captures is not None:
            for capture in self._captures:
                capture.suspend()

    def _resume(self) -> None:
        if self._captures is not None:
            for capture in self._captures:
                capture.resume()

    def _is_started(self) -> bool:
        return self._captures is not None

    @contextmanager
    def disabled(self) -> Iterator[None]:
        with self._engine._uncaptured():
            yield


def _write(stream: Any, text: Any) -> None:
    if isinstance(text, bytes):
        buffer = getattr(stream, "buffer", None)
        if buffer is not None:
            buffer.write(text)
        else:
            stream.write(text.decode("utf-8", "replace"))
    else:
        stream.write(text)
    stream.flush()


# ------------------------------------------------------------------ temporary directories


def _user() -> str | None:
    try:
        return getpass.getuser()
    except (ImportError, OSError, KeyError):
        return None


def _rm_rf(path: Path) -> None:
    def retry(function: Any, failed: str, _: Any) -> None:
        try:
            os.chmod(failed, stat.S_IRWXU)
            function(failed)
        except OSError:
            pass

    if sys.version_info >= (3, 12):
        shutil.rmtree(path, onexc=retry)
    else:
        shutil.rmtree(path, onerror=retry)


def _numbered(root: Path, prefix: str) -> list[int]:
    numbers = []
    try:
        entries = list(os.scandir(root))
    except OSError:
        return numbers
    for entry in entries:
        if entry.name.startswith(prefix):
            suffix = entry.name[len(prefix) :]
            if suffix.isdigit():
                numbers.append(int(suffix))
    return numbers


def _make_numbered_dir(root: Path, prefix: str, mode: int = 0o700) -> Path:
    """``<root>/<prefix><n>`` for the next ``n``, with a ``<prefix>current``
    link to it, as pytest numbers its directories."""
    for _ in range(10):
        number = max(_numbered(root, prefix), default=-1) + 1
        path = root / f"{prefix}{number}"
        try:
            path.mkdir(mode=mode)
        except FileExistsError:
            continue
        link = root / f"{prefix}current"
        try:
            link.unlink()
        except OSError:
            pass
        try:
            link.symlink_to(path)
        except (OSError, NotImplementedError):
            pass
        return path
    raise OSError(f"could not create numbered dir with prefix {prefix} in {root} after 10 tries")


class TempPathFactory:
    """``tmp_path_factory``: numbered directories under one base directory
    per run, which every batch and worker of the run shares, as one pytest
    session would: ``--basetemp`` (emptied once per run; ``popen-<lane>`` in
    it per worker lane) or ``<tmp>/pytest-of-<user>/pytest-<N>``, rotated
    once per run to keep ``retention_count`` of them."""

    def __init__(
        self,
        given_basetemp: Path | str | None = None,
        retention_count: int = 3,
        retention_policy: str = "all",
        *,
        _ispytest: bool = False,
    ) -> None:
        self._given_basetemp = (
            Path(os.path.abspath(str(given_basetemp))) if given_basetemp is not None else None
        )
        self._retention_count = retention_count
        self._retention_policy = retention_policy
        self._basetemp: Path | None = None

    @classmethod
    def from_config(cls, config: Any, *, _ispytest: bool = False) -> TempPathFactory:
        count = int(_ini(config, "tmp_path_retention_count", 3))
        if count < 0:
            raise ValueError(f"tmp_path_retention_count must be >= 0. Current input: {count}.")
        policy = str(_ini(config, "tmp_path_retention_policy", "all"))
        if policy not in ("all", "failed", "none"):
            raise ValueError(
                "tmp_path_retention_policy must be either all, failed, none. "
                f"Current input: {policy}."
            )
        return cls(_option(config, "basetemp"), count, policy)

    def _ensure_relative_to_basetemp(self, basename: str) -> str:
        basename = os.path.normpath(basename)
        base = self.getbasetemp()
        if (base / basename).resolve().parent != base:
            raise ValueError(f"{basename} is not a normalized and relative path")
        return basename

    def mktemp(self, basename: str, numbered: bool = True) -> Path:
        basename = self._ensure_relative_to_basetemp(basename)
        if not numbered:
            path = self.getbasetemp().joinpath(basename)
            path.mkdir(mode=0o700)
            return path
        return _make_numbered_dir(self.getbasetemp(), basename)

    def getbasetemp(self) -> Path:
        if self._basetemp is not None:
            return self._basetemp
        if self._given_basetemp is not None:
            base = self._given_basetemp
            target = base / f"popen-{_LANE}" if _LANE else base
            mark = target / _RUN_MARK
            if _read_text(mark) != _RUN:
                if target.exists():
                    _rm_rf(target)
                target.mkdir(mode=0o700, parents=True)
                mark.write_text(_RUN, encoding="utf-8")
            basetemp = target.resolve()
        else:
            basetemp = self._shared_numbered()
            if _LANE:
                basetemp = basetemp / f"popen-{_LANE}"
                basetemp.mkdir(mode=0o700, exist_ok=True)
        self._basetemp = basetemp
        return basetemp

    def _shared_numbered(self) -> Path:
        temproot = Path(os.environ.get("PYTEST_DEBUG_TEMPROOT") or tempfile.gettempdir()).resolve()
        root = temproot / f"pytest-of-{_user() or 'unknown'}"
        try:
            root.mkdir(mode=0o700, exist_ok=True)
        except OSError:
            root = temproot / "pytest-of-unknown"
            root.mkdir(mode=0o700, exist_ok=True)
        link = root / f"aye-{_RUN}"
        found = _link_target(link)
        if found is not None:
            return found
        keep = 0 if self._retention_policy == "none" else self._retention_count
        made = _make_numbered_dir(root, "pytest-").resolve()
        _lock(made)
        try:
            os.symlink(made, link, target_is_directory=True)
        except FileExistsError:
            found = _link_target(link)
            if found is not None:
                shutil.rmtree(made, ignore_errors=True)
                return found
        except (OSError, NotImplementedError):
            pass
        _rotate(root, keep, made)
        return made


def _read_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, ValueError):
        return None


def _link_target(link: Path) -> Path | None:
    try:
        if link.is_symlink():
            text = os.readlink(link)
            # Windows gives the target as an NT path (\\?\C:\...): spell it
            # as the directory was made, so every session sees one path.
            if os.name == "nt" and text.startswith("\\\\?\\") and text[5:7] == ":\\":
                text = text[4:]
            target = Path(text)
            return target if target.is_dir() else None
    except OSError:
        pass
    return None


# A numbered directory's lock, which a live session holds (pytest's, and
# this engine's); one older than this belongs to a session long gone.
_LOCK = ".lock"
_LOCK_TIMEOUT = 60 * 60 * 24 * 3


def _lock(path: Path) -> None:
    """Hold ``path`` for this process, as pytest does, so that another
    session's rotation leaves it alone; released when the process ends."""
    lock = path / _LOCK
    try:
        lock.write_text(str(os.getpid()), encoding="utf-8")
    except OSError:
        return
    atexit.register(lambda: lock.unlink(missing_ok=True))


def _deletable(path: Path) -> bool:
    """Whether no live session holds ``path``: no lock, or a stale one."""
    try:
        held = (path / _LOCK).stat().st_mtime
    except OSError:
        return True
    return held < time.time() - _LOCK_TIMEOUT


def _rotate(root: Path, keep: int, made: Path) -> None:
    """Remove the oldest ``pytest-<N>`` beyond the newest ``keep`` (the one
    just made always kept) that no live session holds, and links to
    directories that are gone."""
    numbers = sorted(_numbered(root, "pytest-"))
    keep = max(keep, 1)
    for number in numbers[:-keep]:
        path = root / f"pytest-{number}"
        if path != made and _deletable(path):
            shutil.rmtree(path, ignore_errors=True)
    for entry in list(os.scandir(root)):
        if entry.name.startswith("aye-") and entry.is_symlink() and not os.path.exists(entry.path):
            try:
                os.unlink(entry.path)
            except OSError:
                pass


class LocalPath(os.PathLike):
    """The ``tmpdir`` fixture's path: the commonly used part of
    ``py.path.local``, over ``pathlib``."""

    def __init__(self, path: Any = None) -> None:
        self.strpath = os.path.abspath(os.fspath(path) if path is not None else os.getcwd())

    def __fspath__(self) -> str:
        return self.strpath

    def __str__(self) -> str:
        return self.strpath

    def __repr__(self) -> str:
        return f"local({self.strpath!r})"

    def __eq__(self, other: object) -> bool:
        try:
            return os.path.normcase(self.strpath) == os.path.normcase(
                os.path.abspath(os.fspath(other))
            )  # type: ignore[arg-type]
        except TypeError:
            return False

    def __hash__(self) -> int:
        return hash(os.path.normcase(self.strpath))

    def __lt__(self, other: LocalPath) -> bool:
        return self.strpath < os.fspath(other)

    def __truediv__(self, other: Any) -> LocalPath:
        return self.join(os.fspath(other))

    def __div__(self, other: Any) -> LocalPath:
        return self.join(os.fspath(other))

    @property
    def _path(self) -> Path:
        return Path(self.strpath)

    @property
    def basename(self) -> str:
        return os.path.basename(self.strpath)

    @property
    def dirname(self) -> str:
        return os.path.dirname(self.strpath)

    @property
    def purebasename(self) -> str:
        return os.path.splitext(self.basename)[0]

    @property
    def ext(self) -> str:
        return os.path.splitext(self.basename)[1]

    def dirpath(self, *args: str) -> LocalPath:
        return LocalPath(os.path.dirname(self.strpath)).join(*args)

    def join(self, *args: Any) -> LocalPath:
        parts = [os.fspath(arg) for arg in args]
        return LocalPath(os.path.join(self.strpath, *parts)) if parts else LocalPath(self.strpath)

    def new(self, **kwargs: str) -> LocalPath:
        dirname = kwargs.get("dirname", self.dirname)
        basename = kwargs.get("basename")
        if basename is None:
            purebasename = kwargs.get("purebasename", self.purebasename)
            ext = kwargs.get("ext", self.ext)
            if ext and not ext.startswith("."):
                ext = "." + ext
            basename = purebasename + ext
        return LocalPath(os.path.join(dirname, basename))

    def relto(self, other: Any) -> str:
        base = os.fspath(other).rstrip(os.sep) + os.sep
        return self.strpath[len(base) :] if self.strpath.startswith(base) else ""

    def exists(self) -> bool:
        return os.path.exists(self.strpath)

    def isdir(self) -> bool:
        return os.path.isdir(self.strpath)

    def isfile(self) -> bool:
        return os.path.isfile(self.strpath)

    def islink(self) -> bool:
        return os.path.islink(self.strpath)

    def check(self, **kw: Any) -> bool:
        checks = {
            "exists": self.exists,
            "file": self.isfile,
            "dir": self.isdir,
            "link": self.islink,
        }
        if not kw:
            return self.exists()
        for given, wanted in kw.items():
            name = given.removeprefix("not")
            if name not in checks:
                raise TypeError(f"unknown check {given!r}")
            if bool(checks[name]()) != bool(wanted):
                return False
        return True

    def listdir(self, fil: Any = None, sort: Any = None) -> list[LocalPath]:
        entries = [self.join(name) for name in os.listdir(self.strpath)]
        if isinstance(fil, str):
            entries = [e for e in entries if fnmatch.fnmatch(e.basename, fil)]
        elif callable(fil):
            entries = [e for e in entries if fil(e)]
        return sorted(entries, key=lambda e: e.strpath) if sort or sort is None else entries

    def mkdir(self, *args: str) -> LocalPath:
        target = self.join(*args)
        os.mkdir(target.strpath)
        return target

    def ensure(self, *args: str, dir: bool = False) -> LocalPath:
        target = self.join(*args)
        if dir:
            os.makedirs(target.strpath, exist_ok=True)
        else:
            os.makedirs(os.path.dirname(target.strpath), exist_ok=True)
            if not target.exists():
                open(target.strpath, "a").close()
        return target

    def ensure_dir(self, *args: str) -> LocalPath:
        return self.ensure(*args, dir=True)

    def open(self, mode: str = "r", *args: Any, **kwargs: Any) -> Any:
        return open(self.strpath, mode, *args, **kwargs)

    def read(self, mode: str = "r") -> Any:
        with self.open(mode) as file:
            return file.read()

    def read_text(self, encoding: str) -> str:
        return self._path.read_text(encoding=encoding)

    def read_binary(self) -> bytes:
        return self._path.read_bytes()

    def readlines(self, cr: bool = True) -> list[str]:
        text = self.read()
        return text.splitlines(True) if cr else text.splitlines()

    def write(self, data: Any, mode: str = "w", ensure: bool = False) -> None:
        if ensure:
            os.makedirs(self.dirname, exist_ok=True)
        if "b" not in mode and isinstance(data, bytes):
            mode += "b"
        with self.open(mode) as file:
            file.write(data)

    def write_text(self, data: str, encoding: str, ensure: bool = False) -> None:
        if ensure:
            os.makedirs(self.dirname, exist_ok=True)
        self._path.write_text(data, encoding=encoding)

    def write_binary(self, data: bytes, ensure: bool = False) -> None:
        if ensure:
            os.makedirs(self.dirname, exist_ok=True)
        self._path.write_bytes(data)

    def remove(self, rec: int = 1, ignore_errors: bool = False) -> None:
        if self.isdir() and not self.islink():
            if rec:
                shutil.rmtree(self.strpath, ignore_errors=ignore_errors)
            else:
                os.rmdir(self.strpath)
        else:
            os.remove(self.strpath)

    def copy(self, target: Any) -> None:
        if self.isdir():
            shutil.copytree(self.strpath, os.fspath(target), dirs_exist_ok=True)
        else:
            shutil.copy2(self.strpath, os.fspath(target))

    def move(self, target: Any) -> None:
        shutil.move(self.strpath, os.fspath(target))

    def chdir(self) -> LocalPath:
        old = LocalPath()
        os.chdir(self.strpath)
        return old

    @contextmanager
    def as_cwd(self) -> Iterator[LocalPath]:
        old = self.chdir()
        try:
            yield old
        finally:
            os.chdir(old.strpath)

    def realpath(self) -> LocalPath:
        return LocalPath(os.path.realpath(self.strpath))

    def samefile(self, other: Any) -> bool:
        return os.path.samefile(self.strpath, os.fspath(other))

    def stat(self) -> os.stat_result:
        return os.stat(self.strpath)

    def size(self) -> int:
        return os.path.getsize(self.strpath)

    def mtime(self) -> float:
        return os.path.getmtime(self.strpath)

    def visit(self, fil: Any = None) -> Iterator[LocalPath]:
        for directory, dirnames, filenames in os.walk(self.strpath):
            dirnames.sort()
            for name in sorted(dirnames + filenames):
                path = LocalPath(os.path.join(directory, name))
                if fil is None or (fil(path) if callable(fil) else path.basename == fil):
                    yield path

    @property
    def parts(self) -> list[LocalPath]:
        current = self
        chain = [current]
        while current.dirname != current.strpath:
            current = LocalPath(current.dirname)
            chain.append(current)
        return chain[::-1]


# ------------------------------------------------------------------ subtests


class _Subtests:
    """The ``subtests`` resource: ``with subtests.test(msg, **kwargs):``
    runs a block as a sub-outcome of the test; a failed one fails the test
    once it ends, and the block's exception doesn't stop it."""

    def __init__(self, engine: Engine, state: _CaseState) -> None:
        self._engine = engine
        self._state = state

    @contextmanager
    def test(self, msg: str | None = None, **kwargs: Any) -> Iterator[None]:
        engine = self._engine
        began = perf_counter()
        try:
            yield
        except BaseException as exc:
            if isinstance(exc, (KeyboardInterrupt, *engine._exit_types)):
                raise
            name = _subtest_name(msg, kwargs)
            elapsed = perf_counter() - began
            if isinstance(exc, engine._xfail_types):
                engine.sink.sub_outcome(
                    self._state.case.id,
                    name,
                    Outcome.EXPECTED_FAILURE,
                    None,
                    elapsed,
                    _outcome_message(exc) or None,
                )
            elif engine._is_skip(exc):
                engine.sink.sub_outcome(
                    self._state.case.id,
                    name,
                    Outcome.SKIPPED,
                    None,
                    elapsed,
                    aye_engine.skip_reason(exc) or None,
                )
            else:
                self._state.failed_subtests += 1
                failure = engine._failure(exc, Phase.TEST)
                engine.sink.sub_outcome(self._state.case.id, name, Outcome.FAILED, failure, elapsed)
        else:
            engine.sink.sub_outcome(
                self._state.case.id,
                _subtest_name(msg, kwargs),
                Outcome.PASSED,
                None,
                perf_counter() - began,
            )


# ------------------------------------------------------------------ builtins


def builtin_resources(
    config: Any,
    *,
    monkeypatch_class: type | None = None,
    warnings_recorder_class: type | None = None,
) -> dict[str, ResourceDef]:
    """The resources every test can request, visible behind any definition
    of the same name: ``tmp_path``, ``tmp_path_factory``, ``tmpdir``,
    ``tmpdir_factory``, ``monkeypatch`` (an instance of ``monkeypatch_class``,
    undone at teardown), ``capsys``, ``capfd``, ``capsysbinary``,
    ``capfdbinary``, ``caplog``, ``recwarn`` (``warnings_recorder_class``),
    ``request``, ``pytestconfig``, ``record_property``,
    ``record_testsuite_property``, ``record_xml_attribute``, ``subtests``
    and ``doctest_namespace``. ``monkeypatch`` and ``recwarn`` are left out
    without their class."""
    function, session = Scope.FUNCTION, Scope.SESSION
    request = ("request",)
    table: list[tuple[str, Callable[..., Any], Scope, tuple[str, ...]]] = [
        ("tmp_path_factory", functools.partial(_tmp_path_factory, [], config), session, ()),
        ("tmp_path", _tmp_path, function, ("request", "tmp_path_factory")),
        ("tmpdir_factory", _tmpdir_factory, session, ("tmp_path_factory",)),
        ("tmpdir", _tmpdir, function, ("tmp_path",)),
        *(
            (name, _capture_resource(kind), function, request)
            for kind, name in _CAPTURE_NAMES.items()
        ),
        ("caplog", _caplog, function, request),
        ("pytestconfig", _pytestconfig, session, request),
        ("record_property", _record_property, function, request),
        ("record_testsuite_property", _record_testsuite_property, session, request),
        ("record_xml_attribute", _record_xml_attribute, function, request),
        ("subtests", _subtests, function, request),
        ("doctest_namespace", dict, session, ()),
        ("request", _request_resource, function, ()),
    ]
    if monkeypatch_class is not None:
        table.append(
            ("monkeypatch", functools.partial(_monkeypatch, monkeypatch_class), function, ())
        )
    if warnings_recorder_class is not None:
        recwarn = functools.partial(_recwarn, warnings_recorder_class)
        table.append(("recwarn", recwarn, function, ()))
    return {
        name: ResourceDef(
            name=name,
            func=func,
            scope=scope,
            argnames=argnames,
            baseid="",
            location=f"<aye builtin {name}>",
        )
        for name, func, scope, argnames in table
    }


def _tmp_path_factory(made: list[TempPathFactory], config: Any) -> TempPathFactory:
    """One factory per worker, made when first requested: the run's
    directory outlives the batches."""
    if not made:
        made.append(TempPathFactory.from_config(config))
    return made[0]


def _tmp_path(request: Request, tmp_path_factory: TempPathFactory) -> Iterator[Path]:
    name = re.sub(r"[\W]", "_", request.node.name)[:_TMP_NAME_LENGTH]
    path = tmp_path_factory.mktemp(name, numbered=True)
    yield path
    state = request._state
    # Under "failed", pytest keeps it only for a test whose call failed.
    passed = state.call_ok or not state.setup_ok
    if tmp_path_factory._retention_policy == "failed" and passed:
        shutil.rmtree(path, ignore_errors=True)


_TMP_NAME_LENGTH = 30


def _tmpdir_factory(tmp_path_factory: TempPathFactory) -> _TempdirFactory:
    return _TempdirFactory(tmp_path_factory)


def _tmpdir(tmp_path: Path) -> LocalPath:
    return LocalPath(tmp_path)


def _capture_resource(kind: str) -> Callable[[Request], Iterator[CaptureFixture]]:
    own = _CAPTURE_NAMES[kind]

    def capture(request: Request) -> Iterator[CaptureFixture]:
        engine = request._engine
        state = request._state
        others = sorted((set(state.fixturenames) & set(_CAPTURE_NAMES.values())) - {own})
        if others or state.capture_fixture is not None:
            other = (others[0] if len(others) == 1 else others) if others else "another"
            raise FixtureLookupError(
                None, request, f"cannot use {other} and {own} at the same time"
            )
        fixture = CaptureFixture(kind, engine)
        fixture._start()
        state.capture_fixture = engine._capture_fixture = fixture
        try:
            yield fixture
        finally:
            fixture.close()
            state.capture_fixture = engine._capture_fixture = None

    capture.__name__ = own
    return capture


def _caplog(request: Request) -> Iterator[LogCaptureFixture]:
    if request._engine._log is None:
        raise FixtureLookupError("caplog", request, "fixture 'caplog' not found (logging is off)")
    fixture = LogCaptureFixture(request._state.own_item)
    try:
        yield fixture
    finally:
        fixture._finalize()


def _pytestconfig(request: Request) -> Any:
    return request.config


def _record_property(request: Request) -> Callable[[str, object], None]:
    properties = getattr(request._state.item, "user_properties", None)
    if not isinstance(properties, list):
        properties = request._state.own_item.user_properties

    def record_property(name: str, value: object) -> None:
        properties.append((name, value))

    return record_property


def _record_testsuite_property(request: Request) -> Callable[[str, object], None]:
    engine = request._engine

    def record_testsuite_property(name: str, value: object) -> None:
        if not isinstance(name, str):
            raise TypeError(f"name parameter needs to be a string, but {type(name).__name__} given")
        engine.sink.run_property(name, str(value))

    return record_testsuite_property


def _record_xml_attribute(request: Request) -> Callable[[str, object], None]:
    return lambda name, value: None


def _subtests(request: Request) -> _Subtests:
    return _Subtests(request._engine, request._state)


def _request_resource() -> None:
    """Never called: the engine serves ``request`` itself."""


def _monkeypatch(cls: type) -> Iterator[Any]:
    patch = cls()
    try:
        yield patch
    finally:
        patch.undo()


def _recwarn(cls: type) -> Iterator[Any]:
    try:
        recorder = cls(_ispytest=True)
    except TypeError:
        recorder = cls()
    with recorder:
        warnings.simplefilter("default")
        yield recorder


_CAPTURE_NAMES = {
    "sys": "capsys",
    "fd": "capfd",
    "sysbinary": "capsysbinary",
    "fdbinary": "capfdbinary",
}


class _TempdirFactory:
    """``tmpdir_factory``: ``tmp_path_factory`` with ``LocalPath``s."""

    def __init__(self, factory: TempPathFactory) -> None:
        self._factory = factory

    def mktemp(self, basename: str, numbered: bool = True) -> LocalPath:
        return LocalPath(self._factory.mktemp(basename, numbered))

    def getbasetemp(self) -> LocalPath:
        return LocalPath(self._factory.getbasetemp())
