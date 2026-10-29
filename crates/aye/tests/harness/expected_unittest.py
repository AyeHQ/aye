"""Record what plain unittest does on an example suite, in aye's vocabulary.

    python expected_unittest.py <suite-dir> [<output.json>]

The unittest referee: ``record.py`` picks each suite's referee and calls
:func:`record`. It runs ``python -m unittest discover``'s loader and suite in
a subprocess, from the suite's ``[tool.aye.unittest]`` (``start-directory``,
``pattern``, ``top-level-directory``, relative to the suite directory, aye's
project root), and gives {"collected": [ids], "outcomes": {id: outcome},
"exit_code": status} with ids as the unittest adapter names tests:
``path::Class::method``, the path being the module whose loading produced the
test, relative to the suite directory, and ``Class`` the name the class is
bound to in its module (an alias, a class a factory made), else its
qualified name (one ``load_tests`` adds); a ``FunctionTestCase`` is
``path::function`` and a doctest ``path::name``. Collected ids are in
unittest's order; a module that fails to import or skips itself while
imported has none, as aye reports it with a diagnostic. The status is
``python -m unittest``'s (Python 3.12's: 5 when no test ran).

One outcome per test, the worst of its result callbacks, as aye folds them:
success passed, failure failed, error error, skip skipped, expected failure
expected_failure, unexpected success failed (unittest's ``wasSuccessful()`` is
false for it). A failed or errored subtest fails or errors its test; skipped
and passing subtests don't change it, and a test whose only callbacks are
subtests' passed. A test that never ran because its class's or module's setup
failed is error, as aye reports it (unittest reports the setup once instead),
or skipped when that setup raised ``SkipTest``. The comparison harness diffs
aye's results against these files.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile

RECORDER = r"""
import json, os, sys, unittest

ROOT, START, TOP, PATTERN, OUT = sys.argv[1:6]
SEVERITY = ["passed", "skipped", "expected_failure", "unexpected_pass", "failed", "error"]


def flatten(suite, out):
    try:
        items = iter(suite)
    except TypeError:
        out.append(suite)
        return out
    for item in items:
        if item is not None:
            flatten(item, out)
    return out


class Loader(unittest.TestLoader):
    # Which module's discovery produced each test: the module discovery
    # imported, not those a load_tests loads itself. And the name each
    # TestCase test's class was found under in its module (an alias, a class
    # a factory made), as the adapter and pytest name it.
    def __init__(self):
        super().__init__()
        self.origins = {}
        self.discovered = set()
        self.kept = []
        self.bound = {}
        self._names = []

    def _get_module_from_name(self, name):
        module = super()._get_module_from_name(name)
        self.discovered.add(name)
        return module

    def loadTestsFromModule(self, module, *args, pattern=None, **kws):
        self._names.append(bound_names(module))
        try:
            suite = super().loadTestsFromModule(module, *args, pattern=pattern, **kws)
        finally:
            self._names.pop()
        if module.__name__ in self.discovered:
            for test in flatten(suite, []):
                if id(test) not in self.origins:
                    self.origins[id(test)] = module.__name__
                    self.kept.append(test)
        return suite

    def loadTestsFromTestCase(self, testCaseClass):
        suite = super().loadTestsFromTestCase(testCaseClass)
        names = self._names[-1].get(id(testCaseClass)) if self._names else None
        if names:
            # loadTestsFromModule meets the class once per name, in dir() order.
            name = names.pop(0)
            for test in flatten(suite, []):
                self.bound[id(test)] = name
                self.kept.append(test)
        return suite


def bound_names(module):
    # The names loadTestsFromModule finds each TestCase class under.
    names = {}
    for name in dir(module):
        obj = getattr(module, name, None)
        if (
            isinstance(obj, type)
            and issubclass(obj, unittest.TestCase)
            and obj not in (unittest.TestCase, unittest.FunctionTestCase)
        ):
            names.setdefault(id(obj), []).append(name)
    return names


def qualname(obj):
    name = getattr(obj, "__qualname__", None) or getattr(obj, "__name__", repr(obj))
    return obj.__name__ if "<locals>" in name else name


def names(test, bound=None):
    if isinstance(test, unittest.FunctionTestCase):
        return [qualname(test._testFunc)]
    doctest = getattr(test, "_dt_test", None)
    if doctest is not None:
        return [doctest.name]
    if isinstance(test, unittest.TestCase):
        classes = [bound] if bound else qualname(type(test)).split(".")
        return [*classes, test._testMethodName]
    return [str(test.id())]


def kind(test):
    return type(test).__module__, type(test).__name__


class Result(unittest.TestResult):
    def __init__(self, ids):
        super().__init__()
        self.ids = ids
        self.outcomes = {}
        self.started = set()
        self.blocked = {}

    def record(self, test, outcome):
        key = self.ids.get(id(test))
        if key is None:
            return
        old = self.outcomes.get(key, "passed")
        self.outcomes[key] = max(old, outcome, key=SEVERITY.index)

    # Each callback also calls unittest's own, so wasSuccessful() is
    # unittest's verdict.
    def startTest(self, test):
        super().startTest(test)
        self.started.add(id(test))
        self.record(test, "passed")

    def addSuccess(self, test):
        super().addSuccess(test)
        self.record(test, "passed")

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self.record(test, "failed")

    def addError(self, test, err):
        super().addError(test, err)
        if test.__class__.__name__ == "_ErrorHolder":
            self.blocked[test.description] = "error"
        else:
            self.record(test, "error")

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        if test.__class__.__name__ == "_ErrorHolder":
            self.blocked.setdefault(test.description, "skipped")
        elif getattr(test, "test_case", None) is None:
            self.record(test, "skipped")

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        self.record(test, "expected_failure")

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        self.record(test, "failed")

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            failed = issubclass(err[0], test.failureException)
            self.record(test, "failed" if failed else "error")


def main():
    loader = Loader()
    suite = loader.discover(START, PATTERN, TOP)
    collected, ids, tests = [], {}, []
    for test in flatten(suite, []):
        if kind(test) in (("unittest.loader", "_FailedTest"), ("unittest.loader", "ModuleSkipped")):
            continue
        module = sys.modules.get(loader.origins.get(id(test), ""))
        file = os.path.relpath(os.path.abspath(module.__file__), ROOT).replace(os.sep, "/")
        test_id = "::".join([file, *names(test, loader.bound.get(id(test)))])
        collected.append(test_id)
        ids[id(test)] = test_id
        tests.append(test)
    result = Result(ids)
    suite.run(result)
    for test in tests:
        if id(test) in result.started:
            continue
        cls = type(test)
        outcome = "error"
        for where in (f"setUpClass ({cls.__module__}.{cls.__qualname__})", f"setUpModule ({cls.__module__})"):
            if where in result.blocked:
                outcome = result.blocked[where]
                break
        result.record(test, outcome)
    # unittest.main's exit status (5 when nothing ran is Python 3.12's).
    if result.testsRun == 0 and not result.skipped:
        status = 5
    else:
        status = 0 if result.wasSuccessful() else 1
    with open(OUT, "w") as f:
        json.dump({"collected": collected, "outcomes": result.outcomes, "exit_code": status}, f)


main()
"""


def settings(project: str) -> dict[str, str]:
    """``[tool.aye.unittest]`` from the project's ``pyproject.toml``."""
    path = os.path.join(project, "pyproject.toml")
    if not os.path.exists(path):
        return {}
    import tomllib  # noqa: PLC0415 - Python 3.11+, only needed here

    with open(path, "rb") as f:
        return tomllib.load(f).get("tool", {}).get("aye", {}).get("unittest", {})


def record(project: str, env: dict | None = None) -> dict:
    """What plain unittest discovery does on ``project``, run from it in
    ``env`` (default: this process's): {"collected": [ids], "outcomes": {id:
    outcome}, "exit_code": status}, the status being ``python -m
    unittest``'s."""
    project = os.path.realpath(project)
    config = settings(project)
    start = os.path.join(project, config.get("start-directory", "."))
    top = os.path.join(
        project, config.get("top-level-directory", config.get("start-directory", "."))
    )
    pattern = config.get("pattern", "test*.py")
    env = dict(os.environ) if env is None else env
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "unittest.json")
        cmd = [sys.executable, "-c", RECORDER, project, start, top, pattern, out]
        env = dict(env, PYTHONDONTWRITEBYTECODE="1")
        done = subprocess.run(
            cmd, cwd=project, env=env, capture_output=True, text=True, check=False
        )
        if not os.path.exists(out):
            sys.exit(f"unittest in {project} exited {done.returncode}:\n{done.stdout}{done.stderr}")
        with open(out) as f:
            recorded = json.load(f)
    return {
        "collected": recorded["collected"],
        "outcomes": dict(sorted(recorded["outcomes"].items())),
        "exit_code": recorded["exit_code"],
    }


def main() -> None:
    project, *rest = sys.argv[1:]
    result = {"referee": "unittest", **record(project)}
    text = json.dumps(result, indent=2, ensure_ascii=False) + "\n"
    if rest:
        with open(rest[0], "w") as f:
            f.write(text)
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()
