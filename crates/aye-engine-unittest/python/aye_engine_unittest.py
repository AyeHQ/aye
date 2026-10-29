"""The unittest adapter's executor: unittest suites, run natively on aye's engine.

The worker installs this module as ``aye_engine_unittest``, after
``aye_engine``. unittest maps fully onto aye's engine, so the adapter owns the
loop ``TestSuite.run`` would own: the batch's tests run in the order the runner
chose, each through ``TestCase.run`` with the adapter's own ``TestResult``, and
class and module fixtures become aye lifetimes.

- Discovery is ``python -m unittest discover``'s, from the context's
  ``start_directory``, ``top_level_directory`` and ``pattern``. A test
  belongs to a *unit*: its module, loaded as discovery loads it
  (``loadTestsFromModule``, which honours a module's ``load_tests``), or, when
  a package between the start directory and the module may define
  ``load_tests`` (see ``may_define_load_tests``), the outermost such package,
  loaded with ``TestLoader.discover``. The Rust recognizer reads the same
  rule from the same text, so both halves agree on which files a package
  takes over. A file named on the command line (the context's ``named``) is
  its module alone, as ``python -m unittest path.py`` loads it, and the
  tests it loads aren't the package's too, so that each runs once. A file
  the package's ``load_tests`` leaves without a test gets an ``ignored``
  info in refinement, saying why.
- Ids are ``path::Class::method``: the path of the module whose loading
  produced the test (discovery's, not a ``load_tests``' own calls), relative to
  aye's root, then the name the loader found the class under in the module
  (``dir()``: an alias's, a factory-made class's binding), as pytest names
  it, or the class's qualified name split on dots for a class only a
  ``load_tests`` loads, and the method.
  Tests that are not ``TestCase`` methods get ``path::name``: a
  ``FunctionTestCase`` its function's qualified name, a doctest from
  ``DocTestSuite``/``DocFileSuite`` its ``DocTest`` name (``pkg.mod.func``,
  ``README.txt``), anything else its ``id()``.
- A batch loads each unit once, finds every requested id in it (two tests
  with one id both run under it, with a ``duplicate-id`` warning), falls back
  to the module alone and then to ``Class(method)``, and reports what it can't
  find as ``not-found``. A batch re-queued after a crash leaves out the
  tests its context's ``ran_ids`` says ran, by count. A module that fails
  to import (or whose ``load_tests`` raises) is one ``collection-error``,
  which holds every load unittest failed for the file; one that raises
  ``SkipTest`` while imported skips its tests. Loading runs with
  ``os.path``'s own functions in place (``aye_engine.unpatched_os_path``),
  as unittest loads before any test runs.
- Lifetimes follow ``TestSuite``: a module's (``setUpModule``,
  ``tearDownModule``, module cleanups) and a class's (``setUpClass``,
  ``tearDownClass``, class cleanups) open when a test needs them and close when
  the next test doesn't, so a class that comes back is set up again, as in
  unittest. A skipped class runs neither. A failed or skipped setup is reported
  once, on the lifetime; the tests it blocks finish ``error`` (or ``skipped``)
  with the lifetime as their ``cause``; with ``policy.maxfail``, before them.
  A failed teardown or cleanup is the lifetime's failure and fails the batch
  (status 1), as unittest's ``wasSuccessful()``; a ``SkipTest`` there is no
  failure. A ``KeyboardInterrupt`` in a fixture is reported on its lifetime,
  then stops the batch.
- Outcomes come from ``TestResult`` callbacks, the worst one winning:
  ``addSuccess`` passed, ``addFailure`` failed, ``addError`` error,
  ``addSkip`` skipped, ``addExpectedFailure`` expected_failure,
  ``addUnexpectedSuccess`` failed ("unexpected success": unittest's
  ``wasSuccessful()`` is false for it). Every subtest is a ``sub_outcome``,
  and a failed or errored one fails the test with its failure.
- Failures carry the exception, the user's frames (unittest's and aye's
  hidden), the operands of a failed ``assert*`` comparison with unittest's
  explanation as the diff, the phase (``setUp``, the test, ``tearDown`` or a
  cleanup) and unittest's own text as the detail.
- With ``policy.capture``, ``sys.stdout`` and ``sys.stderr`` are captured per
  test, as ``python -m unittest -b`` does, into ``aye_engine.CaptureIO``
  streams (``.buffer``, ``.encoding``, ``fileno()``, readable once closed,
  writing what UTF-8 can't hold as ``?``, as pytest's capture does);
  a class or module fixture's output is kept for its failure (``Stdout:``,
  ``Stderr:`` after its text, as ``-b`` shows it) and dropped when it passes.
  ``policy.maxfail`` stops the batch after that many bad outcomes. A
  ``KeyboardInterrupt`` or a test calling ``result.stop()`` stops it with a
  ``batch-stopped`` error and status 2.
- A test with a bad outcome carries its ``reproduce``: ``aye``, aye's own
  options that shaped the run (the context's ``rerun_args``), the id, then
  ``--`` and the batch's arguments after it (``aye_engine.reproduce``).
- Durations come from ``aye_engine.perf_counter``, capture from
  ``aye_engine.CaptureIO`` and paths from ``aye_engine.os_path``, all bound
  at import, and failures are built so that they never raise: a test that
  patches ``time``, ``io`` or ``os.path`` (and leaves them patched) changes
  nothing the executor reports, in its batch or a later one.

It imports only the standard library, runs on Python 3.10 and later, and never
writes to stdout, which belongs to the worker.
"""

from __future__ import annotations

import fnmatch
import os
import re
import sys
import traceback
import unittest
from collections import Counter
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from typing import Any

import aye_engine
from aye_engine import (
    Failure,
    Lifetime,
    Lifetimes,
    Outcome,
    Phase,
    Result,
    Services,
    Sink,
    perf_counter,
)

# ``os.path`` as it was when the engine was imported (``aye_engine.os_path``):
# tests may patch ``os.path`` and leave it patched.
_path = aye_engine.os_path
_isabs = _path.isabs
_isfile = _path.isfile

ADAPTER = "unittest"

# unittest's ``VALID_MODULE_NAME``: the files discovery may import.
VALID_MODULE_NAME = re.compile(r"[_a-z]\w*\.py$", re.IGNORECASE)

# What makes a package's ``__init__.py`` possibly define ``load_tests``: the
# name itself, or a way to bind names the source doesn't spell (a star
# import, module ``__getattr__``, ``globals()``, ``setattr``...). The Rust
# recognizer applies the same rule to the same bytes.
LOAD_TESTS_MARKERS = re.compile(
    rb"\b(?:load_tests|__getattr__|__dir__|__dict__|__class__|globals|locals|vars|exec|setattr)\b"
    rb"|\bimport[\s\\]*\*"
)

# A ``batch-stopped`` diagnostic names at most this many tests.
MAX_NAMED = 20

# unittest's frames that tell which part of a test raised.
_PHASES = {
    "_callSetUp": Phase.SETUP,
    "_callTestMethod": Phase.TEST,
    "_callTearDown": Phase.TEARDOWN,
    "_callCleanup": Phase.TEARDOWN,
    "doCleanups": Phase.TEARDOWN,
    "subTest": Phase.TEST,
}

# ``assert*`` methods that compare two values: the operator, and the names of
# the operands among their arguments (``None`` for an implicit ``None``).
_COMPARISONS = {
    "assertEqual": ("==", "first", "second"),
    "assertNotEqual": ("!=", "first", "second"),
    "assertAlmostEqual": ("==", "first", "second"),
    "assertNotAlmostEqual": ("!=", "first", "second"),
    # It rebinds ``first`` and ``second`` to ``Counter``s: its copies of the
    # arguments as lists are what the test passed.
    "assertCountEqual": ("==", "first_seq", "second_seq"),
    "assertListEqual": ("==", "list1", "list2"),
    "assertTupleEqual": ("==", "tuple1", "tuple2"),
    "assertSetEqual": ("==", "set1", "set2"),
    "assertDictEqual": ("==", "d1", "d2"),
    "assertSequenceEqual": ("==", "seq1", "seq2"),
    "assertMultiLineEqual": ("==", "first", "second"),
    "assertIn": ("in", "member", "container"),
    "assertNotIn": ("not in", "member", "container"),
    "assertIs": ("is", "expr1", "expr2"),
    "assertIsNot": ("is not", "expr1", "expr2"),
    "assertLess": ("<", "a", "b"),
    "assertLessEqual": ("<=", "a", "b"),
    "assertGreater": (">", "a", "b"),
    "assertGreaterEqual": (">=", "a", "b"),
    "assertIsNone": ("is", "obj", None),
    "assertIsNotNone": ("is not", "obj", None),
    "assertIsInstance": ("isinstance", "obj", "cls"),
    "assertNotIsInstance": ("not isinstance", "obj", "cls"),
}

# The repr of a compared value is cut to this many characters, the bound the
# event schema puts on an assertion's operands.
MAX_REPR = 2000

# The framework's own status for a batch, as ``python -m unittest`` exits.
OK = 0
FAILED = 1
INTERRUPTED = 2
NO_TESTS = 5


def _internal_fragments() -> tuple[str, ...]:
    """unittest's package (and asyncio's, which runs
    ``IsolatedAsyncioTestCase``, and doctest's module, which runs
    ``DocTestSuite``'s cases), plus this module and aye's engine when they
    were loaded from files (the worker installs them from memory, as
    ``<aye:…>``, which ``Paths`` already hides)."""
    package = _path.dirname(_path.abspath(unittest.__file__))
    fragments = [
        package + os.sep,
        _path.join(_path.dirname(package), "asyncio") + os.sep,
        _path.join(_path.dirname(package), "doctest.py"),
    ]
    for code in (_internal_fragments.__code__, aye_engine.Executor.__init__.__code__):
        if not code.co_filename.startswith("<"):
            fragments.append(code.co_filename)
    return tuple(fragments)


def may_define_load_tests(source: bytes) -> bool:
    """Whether a package's ``__init__.py`` may define ``load_tests``, by the
    rule both halves of the adapter share (``LOAD_TESTS_MARKERS``)."""
    return LOAD_TESTS_MARKERS.search(source) is not None


def _context(context: Any) -> dict[str, Any]:
    return context if isinstance(context, dict) else {}


def _under(path: str, directory: str) -> bool:
    """Whether ``path`` is ``directory`` or inside it (both absolute)."""
    return path == directory or path.startswith(directory.rstrip(os.sep) + os.sep)


def _is_package(directory: str) -> bool:
    return _isfile(_path.join(directory, "__init__.py"))


def _ran_ids(context: Any) -> Counter[str]:
    """The context's ``ran_ids`` (added by the runner to a batch it
    re-queued after a crash), counted: an id unittest runs more than once
    (a ``load_tests`` adding a class again) may have run some of its
    tests, and its first that many don't run again."""
    found = _context(context).get("ran_ids")
    if not isinstance(found, list):
        return Counter()
    return Counter(item for item in found if isinstance(item, str))


def _file_of(test_id: str) -> str:
    return test_id.partition("::")[0]


# ------------------------------------------------------------------ discovery


class Layout:
    """Where discovery starts, where imports start, and which files it
    takes, from the recognizer's context."""

    def __init__(self, root: str, context: dict[str, Any]) -> None:
        start = _path.abspath(_path.join(root, context.get("start_directory") or "."))
        top = context.get("top_level_directory")
        top = _path.abspath(_path.join(root, top)) if top else start
        self.pattern: str = context.get("pattern") or "test*.py"
        self.top = top
        # (start, top) pairs: the configured start first, then directories
        # named on the command line outside it, each its own start.
        self.starts = [(start, top)]
        self._discoverable: dict[tuple[str, str, str], bool] = {}
        self._takes_over: dict[str, bool] = {}
        # By file: the unit discovery loads it in (``UnittestExecutor.unit_for``).
        self.units_of: dict[str, tuple[str, str, str, str]] = {}
        for named in context.get("starts") or []:
            extra = _path.abspath(_path.join(root, named))
            self.starts.append((extra, top if _under(extra, top) else extra))
        # Files named on the command line that a package's ``load_tests``
        # would otherwise answer for: each is a module alone, as
        # ``python -m unittest path.py`` loads it.
        self.named = {_path.abspath(_path.join(root, f)) for f in context.get("named") or []}

    def start_for(self, path: str) -> tuple[str, str] | None:
        """The start (and top) discovery reaches ``path`` from: the
        configured one, else the outermost directory named outside it."""
        start, top = self.starts[0]
        if _under(path, start) and path != start:
            return start, top
        found = None
        for start, top in self.starts[1:]:
            if _under(path, start) and path != start and (found is None or _under(found[0], start)):
                found = (start, top)
        return found

    def discoverable(self, path: str, start: str, top: str) -> bool:
        """Whether discovery from ``start`` imports ``path``: a module whose
        name is valid and matches the pattern (``fnmatch``, as unittest), or
        a package's ``__init__.py``, in a directory reached through packages."""
        key = (path, start, top)
        known = self._discoverable.get(key)
        if known is None:
            known = self._discoverable[key] = self._reached(path, start, top)
        return known

    def takes_over(self, package: str) -> bool:
        """Whether a package's ``__init__.py`` may define ``load_tests``."""
        known = self._takes_over.get(package)
        if known is None:
            try:
                with open(_path.join(package, "__init__.py"), "rb") as file:
                    known = may_define_load_tests(file.read())
            except OSError:
                known = False
            self._takes_over[package] = known
        return known

    def _reached(self, path: str, start: str, top: str) -> bool:
        directory, name = _path.split(path)
        if name == "__init__.py":
            if directory == top or not _under(directory, start):
                return False
        elif not (VALID_MODULE_NAME.match(name) and fnmatch.fnmatch(name, self.pattern)):
            return False
        while directory != start:
            if not _is_package(directory):
                return False
            parent = _path.dirname(directory)
            if parent == directory:
                return False
            directory = parent
        return True


def _package_root(path: str) -> str:
    """The first directory upward from ``path`` that is not a package: where
    ``python -m unittest path.py`` would need its name to start."""
    directory = _path.dirname(path)
    while _is_package(directory):
        parent = _path.dirname(directory)
        if parent == directory:
            break
        directory = parent
    return directory


def _module_name(path: str, base: str) -> str:
    """unittest's ``_get_name_from_path``: ``path``'s dotted name from
    ``base``; a package's ``__init__.py`` is the package."""
    stem = _path.splitext(_path.normpath(path))[0]
    if _path.basename(stem) == "__init__":
        stem = _path.dirname(stem)
    return _path.relpath(stem, base).replace(os.sep, ".")


def _module_file(name: str, top: str) -> str | None:
    """The file discovery imported as ``name`` from ``top``."""
    module = sys.modules.get(name)
    file = getattr(module, "__file__", None)
    if isinstance(file, str):
        return _path.abspath(file)
    base = _path.join(top, *name.split("."))
    for candidate in (base + ".py", _path.join(base, "__init__.py")):
        if _isfile(candidate):
            return candidate
    return None


class _Loader(unittest.TestLoader):
    """unittest's loader, remembering which module's loading produced each
    test: the module discovery imported (``_get_module_from_name``), not
    the modules a ``load_tests`` loads itself, so ids name the file
    discovery found the test through."""

    def __init__(self) -> None:
        super().__init__()
        self.origins: dict[int, str] = {}
        self.discovered: set[str] = set()
        # The name each ``TestCase`` test's class was found under in its
        # module's namespace (``dir()``), by test: an alias or a class a
        # factory made is the name it is bound to, as pytest names it.
        self.bound: dict[int, str] = {}
        # For each module being loaded, by class: its names still to meet.
        self._names: list[dict[int, list[str]]] = []
        # Keeps every attributed test alive, so that ``id()`` stays unique.
        self.kept: list[Any] = []

    def _get_module_from_name(self, name: str) -> Any:
        module = super()._get_module_from_name(name)
        self.discovered.add(name)
        return module

    def loadTestsFromModule(self, module: Any, *args: Any, pattern: Any = None, **kws: Any) -> Any:
        self._names.append(_bound_names(module))
        try:
            suite = super().loadTestsFromModule(module, *args, pattern=pattern, **kws)
        finally:
            self._names.pop()
        name = getattr(module, "__name__", None)
        if name in self.discovered:
            for test in flatten(suite):
                if id(test) not in self.origins:
                    self.origins[id(test)] = name
                    self.kept.append(test)
        return suite

    def loadTestsFromTestCase(self, testCaseClass: Any) -> Any:
        suite = super().loadTestsFromTestCase(testCaseClass)
        names = self._names[-1].get(id(testCaseClass)) if self._names else None
        if names:
            # ``loadTestsFromModule`` meets the class once per name, in
            # ``dir()`` order.
            name = names.pop(0)
            for test in flatten(suite):
                self.bound[id(test)] = name
                self.kept.append(test)
        return suite


def _bound_names(module: Any) -> dict[int, list[str]]:
    """The names ``loadTestsFromModule`` finds each ``TestCase`` class
    under, by class, in ``dir()`` order."""
    names: dict[int, list[str]] = {}
    try:
        for name in dir(module):
            obj = getattr(module, name, None)
            if (
                isinstance(obj, type)
                and issubclass(obj, unittest.TestCase)
                and obj not in (unittest.TestCase, unittest.FunctionTestCase)
            ):
                names.setdefault(id(obj), []).append(name)
    except Exception:  # noqa: BLE001 - a namespace that can't be listed: qualified names
        return {}
    return names


def flatten(suite: Any) -> list[Any]:
    """The tests of a suite, depth first, as ``TestSuite.run`` meets them:
    whatever can't be iterated is a test (unittest's ``_isnotsuite``)."""
    out: list[Any] = []
    seen: set[int] = set()

    def walk(item: Any) -> None:
        if item is None:
            return
        try:
            items = iter(item)
        except TypeError:
            out.append(item)
            return
        if id(item) in seen:
            return
        seen.add(id(item))
        for child in items:
            walk(child)

    walk(suite)
    return out


def _is_failed_test(test: Any) -> bool:
    kind = type(test)
    return kind.__name__ == "_FailedTest" and kind.__module__ == "unittest.loader"


def _is_module_skip(test: Any) -> bool:
    kind = type(test)
    return kind.__name__ == "ModuleSkipped" and kind.__module__ == "unittest.loader"


def _is_framework_class(cls: type) -> bool:
    """Classes of unittest and doctest themselves (``FunctionTestCase``,
    ``DocTestCase``): their class and module fixtures do nothing."""
    module = getattr(cls, "__module__", "") or ""
    return module in ("unittest", "doctest") or module.startswith("unittest.")


def _identity(test: Any) -> tuple[type, str] | None:
    """What makes the tests two loads of one module make one test: a
    ``TestCase`` method's class and name; ``None`` for unittest's and
    doctest's own classes (``FunctionTestCase``, ``DocTestCase``), whose
    instances are told apart by more than that."""
    if isinstance(test, unittest.TestCase) and not _is_framework_class(type(test)):
        return type(test), str(test._testMethodName)
    return None


def _qualname(obj: Any) -> str:
    name = getattr(obj, "__qualname__", None) or getattr(obj, "__name__", None)
    if not isinstance(name, str):
        return repr(obj)
    if "<locals>" in name:
        return str(getattr(obj, "__name__", name))
    return name


def _class_names(cls: type) -> list[str]:
    """A class's qualified name, as id parts: ``Outer::Inner``; a class made
    inside a function goes by its own name."""
    return _qualname(cls).split(".")


def names_of(test: Any, bound: str | None = None) -> list[str]:
    """The parts of a test's id after its path: for a ``TestCase`` method,
    the name its class is bound to in the module (``bound``), else the
    class's qualified name, then the method."""
    if isinstance(test, unittest.FunctionTestCase):
        return [_qualname(getattr(test, "_testFunc", None))]
    doctest = getattr(test, "_dt_test", None)
    if doctest is not None and isinstance(test, unittest.TestCase):
        return [str(getattr(doctest, "name", "") or type(test).__name__)]
    if isinstance(test, unittest.TestCase):
        classes = [bound] if bound else _class_names(type(test))
        return [*classes, str(test._testMethodName)]
    ident = getattr(test, "id", None)
    if callable(ident):
        try:
            name = str(ident())
        except Exception:  # noqa: BLE001 - any test-like object will do
            name = ""
        if name:
            return [name]
    return [_qualname(type(test))]


def _unwrap(func: Any) -> Any:
    for _ in range(100):
        inner = getattr(func, "__func__", None) or getattr(func, "__wrapped__", None)
        if inner is None:
            break
        func = inner
    return func


def where(test: Any) -> tuple[str, int | None] | None:
    """Where a test is defined: its method's, function's or doctest's file
    and first line."""
    doctest = getattr(test, "_dt_test", None)
    if doctest is not None:
        filename = getattr(doctest, "filename", None)
        lineno = getattr(doctest, "lineno", None)
        if not isinstance(filename, str):
            return None
        return filename, (lineno + 1 if isinstance(lineno, int) else None)
    if isinstance(test, unittest.FunctionTestCase):
        func = getattr(test, "_testFunc", None)
    elif isinstance(test, unittest.TestCase):
        func = getattr(type(test), test._testMethodName, None)
    else:
        return None
    code = getattr(_unwrap(func), "__code__", None)
    if code is None:
        return None
    return code.co_filename, code.co_firstlineno


class _Entry:
    __slots__ = ("file", "id", "test")

    def __init__(self, test: Any, file: str, test_id: str) -> None:
        self.test = test
        self.file = file
        self.id = test_id


class Unit:
    """What discovery loads for one module, or for one package that may
    define ``load_tests`` (``kind`` ``"module"`` or ``"package"``)."""

    def __init__(self, kind: str, path: str, start: str, top: str) -> None:
        self.kind = kind
        # The module's file, or the package's ``__init__.py``.
        self.path = path
        self.start = start
        self.top = top
        self.entries: list[_Entry] = []
        # By file: why it failed to load (a collection error), or why it
        # skipped itself while imported.
        self.errors: dict[str, str] = {}
        self.skips: dict[str, str] = {}
        # By the file whose refinement reports them (see ``owner``).
        self.owned: dict[str, list[_Entry]] | None = None
        # How many of a package's tests files named on the command line
        # take (``UnittestExecutor._named_tests``).
        self.taken = 0
        self._by_id: dict[str, list[Any]] | None = None

    def tests(self, test_id: str) -> list[Any]:
        """The tests with ``test_id``, in discovery order."""
        if self._by_id is None:
            self._by_id = {}
            for entry in self.entries:
                self._by_id.setdefault(entry.id, []).append(entry.test)
        return self._by_id.get(test_id, [])

    @property
    def directory(self) -> str:
        return _path.dirname(self.path)


# ------------------------------------------------------------------ results


def _safe_repr(value: Any) -> str:
    try:
        text = repr(value)
    except Exception:  # noqa: BLE001 - a broken __repr__ mustn't hide the failure
        text = f"<unrepresentable {type(value).__name__}>"
    return text if len(text) <= MAX_REPR else text[: MAX_REPR - 1] + "…"


def _frames_of(tb: Any) -> list[Any]:
    frames = []
    while tb is not None:
        frames.append(tb.tb_frame)
        tb = tb.tb_next
    return frames


def phase_of(tb: Any) -> Phase:
    """The part of a test that raised, from unittest's own frames."""
    for frame in _frames_of(tb):
        phase = _PHASES.get(frame.f_code.co_name)
        if phase is not None and "__unittest" in frame.f_globals:
            return phase
    return Phase.TEST


def assertion_of(tb: Any, exc: BaseException) -> dict[str, Any] | None:
    """The operands of the ``assert*`` comparison that failed: the first
    comparison method among unittest's frames after the last user frame, read
    from its arguments; unittest's explanation (the message after its first
    line) is the diff. For an equality (but ``assertCountEqual``'s, which
    ignores order), ``compare`` says how the operands differ; neither is
    known as the expected one."""
    frames = _frames_of(tb)
    last_user = -1
    for index, frame in enumerate(frames):
        if "__unittest" not in frame.f_globals:
            last_user = index
    if last_user < 0:
        return None
    for frame in frames[last_user + 1 :]:
        known = _COMPARISONS.get(frame.f_code.co_name)
        if known is None or "__unittest" not in frame.f_globals:
            continue
        op, left_name, right_name = known
        values = frame.f_locals
        if left_name not in values or (right_name is not None and right_name not in values):
            return None
        left = _safe_repr(values[left_name])
        right = "None" if right_name is None else _safe_repr(values[right_name])
        lines = str(exc).splitlines()[1:]
        while lines and not lines[0].strip():
            lines.pop(0)
        while lines and not lines[-1].strip():
            lines.pop()
        assertion: dict[str, Any] = {"op": op, "left": left, "right": right, "diff": lines}
        if op == "==" and right_name is not None and frame.f_code.co_name != "assertCountEqual":
            compared = aye_engine.compare(values[left_name], values[right_name])
            if compared is not None:
                assertion["compare"] = compared
        return assertion
    return None


class _Collector(unittest.TestResult):
    """aye's ``TestResult``: translates one test's callbacks into its
    ``Result`` and its subtests' ``sub_outcome`` events."""

    def __init__(
        self, executor: UnittestExecutor, test: Any, test_id: str, result: Result, sink: Sink
    ) -> None:
        super().__init__()
        self._executor = executor
        self._test = test
        self._id = test_id
        self._result = result
        self._sink = sink

    def _is_subtest(self, test: Any) -> bool:
        return test is not self._test and getattr(test, "test_case", None) is not None

    def addSuccess(self, test: Any) -> None:
        self._result.add(Outcome.PASSED)

    def addFailure(self, test: Any, err: Any) -> None:
        self._result.add(Outcome.FAILED, self._executor.failure(err, test))

    def addError(self, test: Any, err: Any) -> None:
        self._result.add(Outcome.ERROR, self._executor.failure(err, test))

    def addSkip(self, test: Any, reason: str) -> None:
        if self._is_subtest(test):
            name = _subtest_name(test)
            self._sink.sub_outcome(self._id, name, Outcome.SKIPPED, reason=str(reason))
            return
        self._result.add(Outcome.SKIPPED)
        self._result.reason = str(reason)

    def addExpectedFailure(self, test: Any, err: Any) -> None:
        self._result.add(Outcome.EXPECTED_FAILURE)
        exc = err[1] if err and len(err) > 1 else None
        if exc is not None:
            # Never raises (a callback that raised would lose the test).
            self._result.reason = _reason(exc)

    def addUnexpectedSuccess(self, test: Any) -> None:
        self._result.add(Outcome.FAILED, self._executor.unexpected_success(test))

    def addSubTest(self, test: Any, subtest: Any, err: Any) -> None:
        name = _subtest_name(subtest)
        if err is None:
            self._sink.sub_outcome(self._id, name, Outcome.PASSED)
            return
        exception = getattr(test, "failureException", AssertionError)
        failed = isinstance(exception, type) and issubclass(err[0], exception)
        outcome = Outcome.FAILED if failed else Outcome.ERROR
        failure = self._executor.failure(err, test)
        self._sink.sub_outcome(self._id, name, outcome, failure)
        self._result.add(outcome, failure)

    def addDuration(self, test: Any, elapsed: float) -> None:
        pass


def _reason(exc: BaseException) -> str:
    """An expected failure's reason: its exception's message, or its type
    when the exception can't say (a broken ``__str__``)."""
    try:
        return aye_engine.exception_message(exc)
    except Exception:  # noqa: BLE001 - see above
        return type(exc).__name__


def _subtest_name(subtest: Any) -> str:
    """A subtest's description, as unittest prints it: ``[msg] (k=v)``."""
    describe = getattr(subtest, "_subDescription", None)
    try:
        if callable(describe):
            return str(describe())
        return str(subtest.id())
    except Exception:  # noqa: BLE001 - a subtest without a description
        return "(<subtest>)"


@contextmanager
def _nothing() -> Iterator[None]:
    yield


# ------------------------------------------------------------------ lifetimes


class _FixtureLifetime(Lifetime):
    """A class's or a module's fixtures, as ``TestSuite`` runs them.

    With ``capturing`` (the executor's capture, as for a test), what a setup
    or a teardown prints is kept for its failure, as unittest's ``-b`` adds
    it to the fixture's error (``Stdout:``, ``Stderr:``), and dropped when
    it passes. A ``SkipTest`` raised in a teardown or a cleanup is unittest's
    skip, not a failure. A ``KeyboardInterrupt`` is reported as the
    lifetime's failure, then raised again once the lifetime is finished, so
    that the batch stops."""

    capturing: Callable[[Result], Any] | None = None

    def _start(self, sink: Sink) -> None:
        self.is_open = True
        self._started = perf_counter()
        sink.lifetime_started(self.id, self.kind)

    def _capture(self, into: Result) -> Any:
        return self.capturing(into) if self.capturing is not None else _nothing()

    def _setup_failed(self, exc: BaseException, paths: aye_engine.Paths) -> None:
        if aye_engine.is_skip(exc):
            self.skip_reason = aye_engine.skip_reason(exc)
        else:
            self.failure = Failure.from_exception(exc, paths, phase=Phase.SETUP)

    def _report(self, errors: list[BaseException], paths: aye_engine.Paths, phase: Phase) -> None:
        """The first error is the lifetime's failure; the others are added
        to its detail, since a lifetime carries one failure. Skips aren't
        errors (unittest counts them among its skips)."""
        errors = [exc for exc in errors if not aye_engine.is_skip(exc)]
        if not errors:
            return
        rest = errors
        if self.failure is None:
            self.failure = Failure.from_exception(errors[0], paths, phase=phase)
            rest = errors[1:]
        for exc in rest:
            try:
                more = aye_engine.format_exception(exc, paths)
            except Exception:  # noqa: BLE001 - a traceback a test broke
                more = type(exc).__name__
            self.failure.detail = f"{self.failure.detail or ''}\nAlso:\n{more}"

    def _attach(self, output: Result) -> None:
        """What the failed phase printed, after its failure's text, as
        unittest's ``-b`` shows it."""
        if self.failure is None:
            return
        extra = ""
        for label, text in (("Stdout", output.stdout), ("Stderr", output.stderr)):
            if text:
                extra += f"\n{label}:\n{text}" + ("" if text.endswith("\n") else "\n")
        if extra:
            self.failure.detail = (self.failure.detail or "") + extra

    def _own_cleanups(self) -> list[BaseException]:
        errors: list[BaseException] = []
        for fn, args, kwargs in reversed(self._cleanups):
            try:
                fn(*args, **kwargs)
            except BaseException as exc:  # noqa: BLE001 - every cleanup must run
                errors.append(exc)
        self._cleanups.clear()
        return errors

    def _run_setup(
        self,
        setup: Callable[[], Any],
        cleanups: Callable[[], list[BaseException]],
        paths: aye_engine.Paths,
    ) -> bool:
        """Run ``setup``; when it fails, the cleanups at once, as unittest
        does. Returns whether tests inside may run."""
        output = Result(self.id)
        failed: BaseException | None = None
        errors: list[BaseException] = []
        with self._capture(output):
            try:
                setup()
            except BaseException as exc:  # noqa: BLE001 - reported on the lifetime
                failed = exc
                self._setup_failing()
                errors = cleanups()
        if failed is None:
            return True
        self._setup_failed(failed, paths)
        self._report(errors, paths, Phase.SETUP)
        self._attach(output)
        _raise_interrupt([failed, *errors])
        return False

    def _setup_failing(self) -> None:
        """What unittest records when the setup fails, before the cleanups."""

    def _run_teardown(
        self, sink: Sink, paths: aye_engine.Paths, run: Callable[[], list[BaseException]]
    ) -> None:
        """Close: aye's own cleanups, then ``run`` (the teardown and the
        framework's cleanups) when the setup passed, then the report."""
        if not self.is_open:
            return
        self.is_open = False
        failed_before = self.failure is not None
        output = Result(self.id)
        with self._capture(output):
            errors = self._own_cleanups()
            if self.failure is None and self.skip_reason is None:
                errors += run()
        self._report(errors, paths, Phase.TEARDOWN)
        if not failed_before:
            self._attach(output)
        self._finish(sink)
        _raise_interrupt(errors)

    def _finish(self, sink: Sink) -> None:
        sink.lifetime_finished(self.id, self.failure, perf_counter() - self._started)


def _raise_interrupt(errors: list[BaseException]) -> None:
    """Raise the first ``KeyboardInterrupt`` among ``errors``: it stops the
    batch, as it stops unittest."""
    for exc in errors:
        if isinstance(exc, KeyboardInterrupt):
            raise exc


def _calling(fn: Callable[[], Any] | None) -> list[BaseException]:
    if fn is None:
        return []
    try:
        fn()
    except BaseException as exc:  # noqa: BLE001 - reported on the lifetime
        return [exc]
    return []


class ClassLifetime(_FixtureLifetime):
    """``setUpClass``, then ``tearDownClass`` and the class cleanups. A
    skipped class runs neither; a failed ``setUpClass`` runs the cleanups at
    once, as unittest does."""

    def __init__(self, lifetime_id: str, cls: type) -> None:
        super().__init__(lifetime_id, "class")
        self.cls = cls
        self.skipped_class = bool(getattr(cls, "__unittest_skip__", False))

    def open(self, sink: Sink, paths: aye_engine.Paths) -> bool:
        self._start(sink)
        if self.skipped_class:
            return True
        _set(self.cls, "_classSetupFailed", value=False)
        setup = getattr(self.cls, "setUpClass", None)
        if setup is None:
            return True
        return self._run_setup(setup, self._class_cleanups, paths)

    def _setup_failing(self) -> None:
        _set(self.cls, "_classSetupFailed", value=True)

    def _class_cleanups(self) -> list[BaseException]:
        cleanups = getattr(self.cls, "doClassCleanups", None)
        if cleanups is None:
            return []
        try:
            cleanups()
        except BaseException as exc:  # noqa: BLE001 - reported on the lifetime
            return [exc]
        return [info[1] for info in getattr(self.cls, "tearDown_exceptions", None) or ()]

    def close(self, sink: Sink, paths: aye_engine.Paths) -> None:
        def run() -> list[BaseException]:
            if self.skipped_class:
                return []
            return _calling(getattr(self.cls, "tearDownClass", None)) + self._class_cleanups()

        self._run_teardown(sink, paths, run)


class ModuleLifetime(_FixtureLifetime):
    """``setUpModule``, then ``tearDownModule`` and the module cleanups
    (``unittest.addModuleCleanup``). A failed ``setUpModule`` runs the
    cleanups at once, as unittest does."""

    def __init__(self, lifetime_id: str, module_name: str) -> None:
        super().__init__(lifetime_id, "module")
        self.module_name = module_name

    def _fixture(self, name: str) -> Callable[[], Any] | None:
        module = sys.modules.get(self.module_name)
        return getattr(module, name, None) if module is not None else None

    def open(self, sink: Sink, paths: aye_engine.Paths) -> bool:
        self._start(sink)
        setup = self._fixture("setUpModule")
        if setup is None:
            return True
        return self._run_setup(setup, _module_cleanups, paths)

    def close(self, sink: Sink, paths: aye_engine.Paths) -> None:
        def run() -> list[BaseException]:
            return _calling(self._fixture("tearDownModule")) + _module_cleanups()

        self._run_teardown(sink, paths, run)


def _module_cleanups() -> list[BaseException]:
    try:
        unittest.case.doModuleCleanups()
    except BaseException as exc:  # noqa: BLE001 - reported on the lifetime
        return [exc]
    return []


def _set(cls: type, name: str, *, value: bool) -> None:
    try:
        setattr(cls, name, value)
    except (TypeError, AttributeError):  # a builtin type, or a read-only class
        pass


# ------------------------------------------------------------------ the executor


class UnittestExecutor(aye_engine.Executor):
    """Runs batches and refines files of unittest suites, natively. It runs
    its own loop rather than ``EngineExecutor``'s, since unittest decides
    outcomes through ``TestResult`` callbacks rather than by what the test
    raised, but it uses the engine's ``Lifetimes``, ``Result`` and
    ``Failure``."""

    adapter = ADAPTER
    internal = _internal_fragments()
    # aye's own options a rerun repeats, for the batch running now.
    rerun_args: Sequence[str] = ()

    # ------------------------------------------------------------ units

    def absolute(self, path: str) -> str:
        """A project-relative path (``/`` separators), made absolute."""
        return _path.normpath(_path.join(self.root, path))

    def unit_for(self, path: str, layout: Layout, units: dict[tuple[str, str], Unit]) -> Unit:
        """The unit discovery loads ``path`` (absolute) in, loaded once per
        request: the outermost package between the start directory and the
        file that may define ``load_tests``, else the module."""
        known = layout.units_of.get(path)
        if known is None:
            known = self._locate(path, layout)
            layout.units_of[path] = known
        kind, unit_path, start, top = known
        return self._unit(kind, unit_path, start, top, layout, units)

    def _locate(self, path: str, layout: Layout) -> tuple[str, str, str, str]:
        if path in layout.named:
            return self._module_place(path, layout)
        found = layout.start_for(path)
        if found is not None and layout.discoverable(path, *found):
            start, top = found
            chain = []
            current = _path.dirname(path)
            while _under(current, start):
                chain.append(current)
                if current == start:
                    break
                current = _path.dirname(current)
            for package in reversed(chain):
                if package == top or not _is_package(package):
                    continue
                if layout.takes_over(package):
                    return "package", _path.join(package, "__init__.py"), start, top
            return "module", path, start, top
        return self._module_place(path, layout)

    def _module_place(self, path: str, layout: Layout) -> tuple[str, str, str, str]:
        found = layout.start_for(path)
        if found is not None and layout.discoverable(path, *found):
            return "module", path, found[0], found[1]
        top = layout.top
        if not _under(path, top) or _module_name(path, top) == ".":
            top = _package_root(path)
        return "module", path, _path.dirname(path), top

    def module_unit(self, path: str, layout: Layout, units: dict[tuple[str, str], Unit]) -> Unit:
        """The module alone: as discovery loads it when it may, else as
        ``python -m unittest path.py`` does, from its package root."""
        kind, unit_path, start, top = self._module_place(path, layout)
        return self._unit(kind, unit_path, start, top, layout, units)

    def _unit(
        self,
        kind: str,
        path: str,
        start: str,
        top: str,
        layout: Layout,
        units: dict[tuple[str, str], Unit],
    ) -> Unit:
        key = (kind, path)
        unit = units.get(key)
        if unit is None:
            unit = Unit(kind, path, start, top)
            units[key] = unit
            if kind == "package":
                self._load_package(unit, layout)
            else:
                self._load_module(unit, layout)
        return unit

    def _load_module(self, unit: Unit, layout: Layout) -> None:
        """One module, as ``_find_test_path`` loads it: imported by its name
        from the top-level directory, checked to be the file asked for, then
        ``loadTestsFromModule`` (which calls its ``load_tests``)."""
        path = unit.path
        loader = _Loader()
        name = _module_name(path, unit.top)
        if unit.top not in sys.path:
            sys.path.insert(0, unit.top)
        try:
            module = loader._get_module_from_name(name)
        except unittest.SkipTest as exc:
            unit.skips[path] = str(exc)
            return
        except (Exception, SystemExit) as exc:  # noqa: BLE001 - reported as unittest reports it
            unit.errors[path] = f"Failed to import test module: {name}\n{self._format(exc)}"
            return
        problem = _misplaced(module, path)
        if problem is not None:
            unit.errors[path] = problem
            return
        found = layout.start_for(path)
        discovered = found is not None and layout.discoverable(path, *found)
        try:
            suite = loader.loadTestsFromModule(
                module, pattern=layout.pattern if discovered else None
            )
        except (Exception, SystemExit) as exc:  # noqa: BLE001 - reported as a collection error
            unit.errors[path] = f"Failed to load tests from {name}\n{self._format(exc)}"
            return
        self._entries(unit, loader, suite)

    def _load_package(self, unit: Unit, layout: Layout) -> None:
        """A package that may define ``load_tests``, as discovery from it
        loads it: ``TestLoader.discover``."""
        loader = _Loader()
        try:
            suite = loader.discover(unit.directory, layout.pattern, unit.top)
        except (Exception, SystemExit) as exc:  # noqa: BLE001 - reported as a collection error
            name = _module_name(unit.path, unit.top)
            unit.errors[unit.path] = f"Failed to discover tests in {name}\n{self._format(exc)}"
            return
        self._entries(unit, loader, suite)

    def _entries(self, unit: Unit, loader: _Loader, suite: Any) -> None:
        for test in flatten(suite):
            if _is_failed_test(test):
                name = str(getattr(test, "_testMethodName", "") or "")
                file = _module_file(name, unit.top) if name else None
                file = file if file and _under(file, unit.directory) else unit.path
                exc = getattr(test, "_exception", None)
                message = self._failed_load(exc, name)
                known = unit.errors.get(file)
                # unittest reports each failed load: so does the file's error.
                if known is None:
                    unit.errors[file] = message
                elif message not in known.split("\n\n"):
                    unit.errors[file] = f"{known}\n\n{message}"
                continue
            if _is_module_skip(test):
                name = str(getattr(test, "_testMethodName", "") or "")
                file = _module_file(name, unit.top) or unit.path
                method = getattr(test, name, None)
                unit.skips.setdefault(file, str(getattr(method, "__unittest_skip_why__", "")))
                continue
            origin = loader.origins.get(id(test))
            file = (_module_file(origin, unit.top) if origin else None) or unit.path
            bound = loader.bound.get(id(test))
            unit.entries.append(_Entry(test, file, self.test_id(test, file, bound)))

    def _failed_load(self, exc: BaseException | None, name: str = "") -> str:
        """unittest's message for a ``_FailedTest``: an import that failed,
        a ``load_tests`` that raised (the test is named after the module), or
        a name ``loadTestsFromName`` couldn't reach."""
        if exc is None:
            return "unittest failed to load this module"
        try:
            text = str(exc)
        except Exception:  # noqa: BLE001 - a broken __str__
            text = ""
        if isinstance(exc, ImportError) and text.startswith("Failed to import test module"):
            return text
        if name and name not in sys.modules:
            return f"Failed to access attribute:\n{self._format(exc)}"
        return f"Failed to call load_tests:\n{self._format(exc)}"

    def _format(self, exc: BaseException) -> str:
        """``exc`` with its traceback, without the frames of the runner, the
        engine, unittest or the import system."""
        tb = traceback.TracebackException.from_exception(exc)
        kept = [frame for frame in tb.stack if not self.paths.is_internal(frame.filename)]
        tb.stack = traceback.StackSummary.from_list(kept)
        return "".join(tb.format())

    def owner(self, entry: _Entry, unit: Unit, layout: Layout) -> str:
        """The file whose refinement reports a test: in a module's unit, that
        module; in a package's, the module discovery found it in, when that
        is a file discovery from the package takes, else the package's
        ``__init__.py``."""
        if unit.kind == "module":
            return unit.path
        found = layout.start_for(entry.file)
        if (
            _under(entry.file, unit.directory)
            and found is not None
            and layout.discoverable(entry.file, *found)
        ):
            return entry.file
        return unit.path

    # ------------------------------------------------------------ ids and cases

    def test_id(self, test: Any, file: str, bound: str | None = None) -> str:
        return "::".join([self.paths.relative(file), *names_of(test, bound)])

    def case(self, entry: _Entry, unit: Unit) -> dict[str, Any]:
        path, _, rest = entry.id.partition("::")
        names = rest.split("::")
        span: dict[str, Any] = {"path": path, "start": 0, "end": 0}
        found = where(entry.test)
        if found is not None:
            file, line = found
            relative = self.paths.relative(file)
            if not _isabs(relative):
                span["path"] = relative
                if line is not None:
                    span["line"] = line
        affinity = [path]
        if isinstance(entry.test, unittest.TestCase) and len(names) > 1:
            affinity.append(f"{path}::{'::'.join(names[:-1])}")
        if unit.kind == "package":
            affinity.insert(0, f"package:{self.paths.relative(unit.directory)}")
        keywords = [part for part in path.split("/") if part]
        keywords += [name for name in names if name]
        return {
            "id": entry.id,
            "adapter": ADAPTER,
            "span": span,
            "keywords": list(dict.fromkeys(keywords)),
            "affinity": affinity,
            "certainty": "refined",
        }

    # ------------------------------------------------------------ refinement

    def refine(self, files: list[str], context: Any) -> dict[str, Any]:
        """Load ``files`` (project-relative) as discovery would and return
        their tests as cases (``certainty: refined``); import errors are
        ``collection-error`` diagnostics, and modules that skip themselves
        while imported ``skipped`` infos."""
        layout = Layout(self.root, _context(context))
        units: dict[tuple[str, str], Unit] = {}
        cases: list[dict[str, Any]] = []
        diagnostics: list[dict[str, Any]] = []

        def diagnostic(severity: str, code: str, path: str, message: str) -> None:
            message = aye_engine.bound(message)[0]
            diagnostics.append(
                {"severity": severity, "code": code, "path": path, "message": message}
            )

        for file in dict.fromkeys(files):
            path = self.absolute(file)
            if not _isfile(path):
                diagnostic("error", "not-found", file, f"{file}: no such file")
                continue
            with aye_engine.unpatched_os_path():
                unit = self.unit_for(path, layout, units)
                if unit.owned is None:
                    taken = self._named_tests(unit, layout, units)
                    unit.owned = {}
                    for entry in unit.entries:
                        if _identity(entry.test) in taken:
                            unit.taken += 1
                            continue
                        unit.owned.setdefault(self.owner(entry, unit, layout), []).append(entry)
            owned = unit.owned.get(path, ())
            cases += [self.case(entry, unit) for entry in owned]
            if not owned:
                why = self._why_none(path, unit, layout)
                if why is not None:
                    diagnostic("info", "ignored", file, why)
            for where_, message in unit.errors.items():
                if self._owned(where_, unit, layout) == path:
                    diagnostic("error", "collection-error", self.paths.relative(where_), message)
            for where_, reason in unit.skips.items():
                if self._owned(where_, unit, layout) == path:
                    diagnostic(
                        "info",
                        "skipped",
                        self.paths.relative(where_),
                        reason or "skipped while imported",
                    )
        return aye_engine.sanitize({"cases": cases, "diagnostics": diagnostics})

    def _owned(self, file: str, unit: Unit, layout: Layout) -> str:
        return self.owner(_Entry(None, file, ""), unit, layout)

    def _why_none(self, path: str, unit: Unit, layout: Layout) -> str | None:
        """Why a file a package's discovery answers for has no test, when
        the package's ``load_tests`` decided it (an ``ignored`` info, in
        place of collection's "has none"): it loads none from the file, or
        its tests run under the package's ``__init__.py``, or under files
        named on the command line."""
        if unit.kind != "package" or any(
            place in unit.errors or place in unit.skips for place in (path, unit.path)
        ):
            return None
        init = self.paths.relative(unit.path)
        if path == unit.path:
            if not unit.taken:
                return None
            return (
                "the tests its load_tests loads are in files named on the command line, "
                "which run them"
            )
        if self._owned(path, unit, layout) != path:
            return None
        decides = f"{init} may define load_tests, which decides the tests below it"
        for entry in unit.entries:
            found = where(entry.test)
            if found is not None and _path.abspath(found[0]) == path:
                return f"{decides}: those it loads from this file run under {init}"
        return f"{decides}, and loads none from this file"

    def _named_tests(
        self, unit: Unit, layout: Layout, units: dict[tuple[str, str], Unit]
    ) -> set[tuple[type, str]]:
        """The tests that files named on the command line below a package's
        directory make, each loaded alone (``layout.named``): the package's
        discovery leaves them to those files, so that a test both the
        directory and a file in it name runs once, under the file's id, as
        pytest runs it (unittest, given both names, runs it twice)."""
        taken: set[tuple[type, str]] = set()
        if unit.kind != "package":
            return taken
        for named in sorted(layout.named):
            if named == unit.path or not _under(named, unit.directory) or not _isfile(named):
                continue
            for entry in self.module_unit(named, layout, units).entries:
                key = _identity(entry.test)
                if key is not None:
                    taken.add(key)
        return taken

    # ------------------------------------------------------------ running

    def run(self, batch: aye_engine.Batch, sink: Sink) -> int:  # noqa: PLR0912, PLR0915 - one loop, top to bottom
        """Run ``batch``: load what its ids need, then run each test in the
        batch's order, opening and closing class and module lifetimes as the
        order requires. Returns ``python -m unittest``'s status: 0, 1 when a
        test failed or a module didn't load, 2 when stopped, 5 when nothing
        ran."""
        if not batch.ids:
            return NO_TESTS
        self.rerun_args = aye_engine.rerun_args(batch.context)
        layout = Layout(self.root, _context(batch.context))
        try:
            with aye_engine.unpatched_os_path():
                plan, problems = self._plan(batch.ids, layout, sink, _ran_ids(batch.context))
        except KeyboardInterrupt:
            message = "the batch stopped early: KeyboardInterrupt while importing its tests"
            sink.diagnostic("error", "batch-stopped", _file_of(batch.ids[0]), message)
            return INTERRUPTED
        lifetimes = Lifetimes(sink, self.paths)
        context = self.services.get(Services.CONTEXT)
        cleanup = self.services.get(Services.CLEANUP)
        maxfail = batch.policy.maxfail
        bad = ran = 0
        started: set[int] = set()
        stopped: str | None = None
        needs = _Needs(self, lambda into: self._capture(into, batch.policy))
        try:
            for index, (test_id, test, skip) in enumerate(plan):
                if maxfail and bad >= maxfail:
                    break
                blocker = lifetimes.enter(needs.of(test) if skip is None else needs.none())
                lifetimes.report_early(blocker, maxfail)
                started.add(index)
                if context is not None:
                    context.test = test_id
                    context.lifetimes = [lifetime.id for lifetime in lifetimes.open]
                result, stop = self._run_test(
                    test_id, test, skip, blocker, sink, batch.policy, cleanup
                )
                ran += 1
                if result.outcome is not None and result.outcome.is_bad:
                    bad += 1
                if stop:
                    stopped = "a test asked unittest to stop (result.stop())"
                    break
        except KeyboardInterrupt:
            stopped = "KeyboardInterrupt"
        finally:
            # Each close pops its lifetime first: an interrupt in one
            # teardown still lets the others close.
            while lifetimes.open:
                try:
                    lifetimes.close_all()
                except KeyboardInterrupt:
                    stopped = stopped or "KeyboardInterrupt"
            if context is not None:
                context.test = None
                context.lifetimes = []
        if stopped is not None:
            left = [test_id for i, (test_id, _, _) in enumerate(plan) if i not in started]
            named = ", ".join(left[:MAX_NAMED]) + (" …" if len(left) > MAX_NAMED else "")
            message = f"the batch stopped early: {stopped}"
            if left:
                message += f"; {len(left)} test(s) never started: {named}"
            sink.diagnostic(
                "error", "batch-stopped", _file_of(plan[0][0]) if plan else ".", message
            )
            return INTERRUPTED
        # unittest's ``wasSuccessful()`` is false after a failed
        # ``tearDownClass``, ``tearDownModule`` or class or module cleanup too.
        if bad or problems or needs.failed():
            return FAILED
        return OK if ran else NO_TESTS

    def _plan(
        self, ids: list[str], layout: Layout, sink: Sink, ran: Counter[str] | None = None
    ) -> tuple[list[tuple[str, Any, str | None]], bool]:
        """Each requested id's tests, in the batch's order, as ``(id, test,
        skip reason)``, but the first ``ran[id]`` of them, which ran before
        the worker of the batch this one retries crashed; and whether any
        id couldn't be loaded."""
        ran = ran if ran is not None else Counter()
        units: dict[tuple[str, str], Unit] = {}
        plan: list[tuple[str, Any, str | None]] = []
        reported: set[tuple[str, str]] = set()
        problems = False
        for test_id in dict.fromkeys(ids):
            tests, skip, error = self._resolve(test_id, layout, units)
            if error is not None:
                problems = True
                path, code, message = error
                if (code, path) not in reported or code == "not-found":
                    reported.add((code, path))
                    sink.diagnostic("error", code, path, message)
                continue
            if skip is not None:
                if not ran[test_id]:
                    plan.append((test_id, None, skip))
                continue
            if len(tests) > 1:
                sink.diagnostic(
                    "warning",
                    "duplicate-id",
                    _file_of(test_id),
                    f"unittest runs more than one test with the id `{test_id}`; each one's "
                    "result is reported under that id, so it has more results than "
                    "collection lists",
                )
            plan += [(test_id, test, None) for test in tests[ran[test_id] :]]
        return plan, problems

    def _resolve(
        self, test_id: str, layout: Layout, units: dict[tuple[str, str], Unit]
    ) -> tuple[list[Any], str | None, tuple[str, str, str] | None]:
        """The tests a requested id names: in its unit, else in its module
        alone, else ``Class(method)`` as ``loadTestsFromName`` makes it.
        Returns the tests, a skip reason for a module that skipped itself, or
        an error ``(path, code, message)``."""
        path, sep, rest = test_id.partition("::")
        file = self.absolute(path)
        if not sep or not _isfile(file):
            return [], None, (path, "not-found", f"{test_id}: no such file")
        unit = self.unit_for(file, layout, units)
        candidates = [unit]
        if unit.kind == "package":
            candidates.append(self.module_unit(file, layout, units))
        for candidate in candidates:
            tests = candidate.tests(test_id)
            if tests:
                return tests, None, None
            if file in candidate.skips:
                return [], candidate.skips[file] or "skipped while imported", None
            if file in candidate.errors:
                return [], None, (path, "collection-error", candidate.errors[file])
        direct = self._direct(file, rest.split("::"), units)
        if direct is not None:
            return [direct], None, None
        return [], None, (path, "not-found", f"{test_id}: unittest found no such test")

    def _direct(self, file: str, names: list[str], units: dict[tuple[str, str], Unit]) -> Any:
        """``Class(method)`` from the module already imported for ``file``,
        as ``loadTestsFromName`` makes a test of a method it names."""
        for (kind, path), unit in units.items():
            if kind != "module" or path != file or unit.errors or unit.skips:
                continue
            module = sys.modules.get(_module_name(file, unit.top))
            if module is None or len(names) < 2:  # noqa: PLR2004 - a class and a method
                return None
            owner: Any = module
            for name in names[:-1]:
                owner = getattr(owner, name, None)
            method = names[-1]
            if (
                isinstance(owner, type)
                and issubclass(owner, unittest.TestCase)
                and hasattr(owner, method)
            ):
                try:
                    return owner(method)
                except Exception:  # noqa: BLE001 - not a test after all
                    return None
        return None

    def _run_test(
        self,
        test_id: str,
        test: Any,
        skip: str | None,
        blocker: Lifetime | None,
        sink: Sink,
        policy: aye_engine.Policy,
        cleanup: Any,
    ) -> tuple[Result, bool]:
        sink.test_started(test_id)
        result = Result(test_id)
        began = perf_counter()
        stop = False
        try:
            if skip is not None:
                result.add(Outcome.SKIPPED)
                result.reason = skip
            elif blocker is not None:
                if blocker.skip_reason is not None:
                    result.add(Outcome.SKIPPED)
                    result.reason = blocker.skip_reason
                else:
                    result.add(Outcome.ERROR)
                    result.cause = blocker.id
            else:
                stop = self._call(test_id, test, result, sink, policy, cleanup)
        finally:
            result.duration = perf_counter() - began
            if result.outcome is not None and result.outcome.is_bad:
                result.reproduce = aye_engine.reproduce(
                    test_id, policy.framework_args, self.rerun_args
                )
            sink.test_finished(result)
        return result, stop

    def _call(
        self,
        test_id: str,
        test: Any,
        result: Result,
        sink: Sink,
        policy: aye_engine.Policy,
        cleanup: Any,
    ) -> bool:
        """Run one test through ``test(result)``, as ``TestSuite`` calls it,
        with aye's cleanup scope around it. Returns whether the test asked
        unittest to stop."""
        collector = _Collector(self, test, test_id, result, sink)
        if cleanup is not None:
            cleanup.push()
        try:
            with self._capture(result, policy):
                try:
                    test(collector)
                except KeyboardInterrupt as exc:
                    failure = Failure.from_exception(exc, self.paths, phase=Phase.TEST)
                    result.add(Outcome.ERROR, failure)
                    raise
                except BaseException as exc:  # noqa: BLE001 - what the test raised is its outcome
                    failure = Failure.from_exception(exc, self.paths, phase=Phase.TEST)
                    result.add(Outcome.ERROR, failure)
        finally:
            if cleanup is not None:
                error = cleanup.pop()
                if error is not None:
                    result.add(
                        Outcome.ERROR,
                        Failure.from_exception(error, self.paths, phase=Phase.TEARDOWN),
                    )
                    if isinstance(error, KeyboardInterrupt):
                        raise error
        return bool(collector.shouldStop)

    def _capture(self, result: Result, policy: aye_engine.Policy) -> Any:
        if not policy.capture:
            return _nothing()
        service = self.services.get(Services.CAPTURE)
        if service is not None:
            return service.capture(result)
        return aye_engine.captured(result)

    # ------------------------------------------------------------ failures

    def failure(self, err: Any, test: Any) -> Failure:
        """A structured failure from unittest's ``exc_info``: read before
        unittest's own text is made, since making it trims the traceback.
        It never raises (a result callback that raised would lose the test):
        what can't be read is left out."""
        exc_type, exc, tb = err
        if exc is None:
            exc = exc_type() if isinstance(exc_type, type) else RuntimeError(str(exc_type))
        try:
            return self._failure(exc, tb, err, test)
        except Exception:  # noqa: BLE001 - see above
            try:
                phase = phase_of(tb)
            except Exception:  # noqa: BLE001 - see above
                phase = Phase.TEST
            return Failure.from_exception(exc, self.paths, phase=phase)

    def _failure(self, exc: BaseException, tb: Any, err: Any, test: Any) -> Failure:
        frames = aye_engine.user_frames(tb, self.paths)
        location = (frames[-1]["path"], frames[-1]["line"]) if frames else None
        assertion = assertion_of(tb, exc)
        phase = phase_of(tb)
        try:
            detail = self._exc_info_to_string(err, test)
        except Exception:  # noqa: BLE001 - fall back to Python's own text
            try:
                detail = aye_engine.format_exception(exc, self.paths)
            except Exception:  # noqa: BLE001 - a traceback module a test broke
                detail = None
        return Failure(
            aye_engine.exception_message(exc),
            phase=phase,
            exception=type(exc).__name__,
            assertion=assertion,
            location=location,
            frames=frames,
            detail=detail,
        )

    def _exc_info_to_string(self, err: Any, test: Any) -> str:
        return unittest.TestResult()._exc_info_to_string(err, test)

    def module_lifetime_id(self, module: str) -> str:
        """A module's lifetime: its file relative to the root (the first
        affinity key of its tests), else ``module:NAME``."""
        file = getattr(sys.modules.get(module), "__file__", None)
        if isinstance(file, str):
            relative = self.paths.relative(file)
            if not _isabs(relative):
                return relative
        return f"module:{module}"

    def unexpected_success(self, test: Any) -> Failure:
        """The failure of a test expected to fail that passed: at its
        definition, since nothing raised."""
        location = None
        found = where(test)
        if found is not None and found[1] is not None:
            relative = self.paths.relative(found[0])
            if not _isabs(relative):
                location = (relative, found[1])
        return Failure("unexpected success", phase=Phase.TEST, location=location)


class _Needs:
    """The lifetimes each test needs, outermost first, made as ``TestSuite``
    meets classes and modules: a new lifetime whenever the class (or the
    module) differs from the previous test's, so one that comes back is set
    up again."""

    def __init__(
        self, executor: UnittestExecutor, capturing: Callable[[Result], Any] | None = None
    ) -> None:
        self.executor = executor
        self.capturing = capturing
        self.module: str | None = None
        self.cls: Any = None
        self.lifetimes: list[Lifetime] = []
        # Every lifetime made, to tell whether one failed.
        self.made: list[_FixtureLifetime] = []

    def failed(self) -> bool:
        """Whether a lifetime failed, in its setup or its teardown."""
        return any(lifetime.failure is not None for lifetime in self.made)

    def _made(self, lifetime: _FixtureLifetime) -> _FixtureLifetime:
        lifetime.capturing = self.capturing
        self.made.append(lifetime)
        return lifetime

    def none(self) -> list[Lifetime]:
        """No lifetimes, for a test that won't run: the next one starts
        afresh, as after another module."""
        self.module, self.cls, self.lifetimes = None, None, []
        return []

    def of(self, test: Any) -> list[Lifetime]:
        cls = type(test)
        if _is_framework_class(cls) or not isinstance(test, unittest.TestCase):
            return self.none()
        module = getattr(cls, "__module__", None) or ""
        if module != self.module or not self.lifetimes:
            self.lifetimes = [
                self._made(ModuleLifetime(self.executor.module_lifetime_id(module), module))
            ]
            self.cls = None
        if cls is not self.cls:
            class_id = f"{self.lifetimes[0].id}::{'::'.join(_class_names(cls))}"
            self.lifetimes = [self.lifetimes[0], self._made(ClassLifetime(class_id, cls))]
        self.module, self.cls = module, cls
        return list(self.lifetimes)


def _misplaced(module: Any, path: str) -> str | None:
    """unittest's check that discovery imported the file it found, rather
    than a module of the same name elsewhere."""
    file = getattr(module, "__file__", None)
    if not isinstance(file, str):
        return None
    real = _path.splitext(_path.realpath(_path.abspath(file)))[0]
    wanted = _path.splitext(_path.realpath(path))[0]
    if real.lower() == wanted.lower():
        return None
    name = _path.splitext(_path.basename(path))[0]
    return (
        f"{name!r} module incorrectly imported from {_path.dirname(real)!r}. "
        f"Expected {_path.dirname(path)!r}. Is this module globally installed?"
    )
