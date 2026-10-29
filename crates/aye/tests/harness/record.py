"""Record what a project's referee does on it, for the comparison harness.

    python record.py <suite-dir> [<output.json>]
    python record.py --example <example-dir> [<output.json>]

``make expected`` runs it on every suite in ``crates/aye/tests/suites`` (into
each suite's ``expected.json``) and every project in ``examples/`` (into
``crates/aye/tests/examples/<name>.json``); ``compare.rs`` runs aye on a fresh
copy of each and diffs. Needs Python 3.11 or later (``tomllib``).

The referee is the framework's own runner, chosen from the project's
``[tool.aye] adapters``:

- plain unittest (``expected_unittest.py``, with ``[tool.aye.unittest]``)
  when unittest is the only adapter;
- otherwise plain pytest (``expected.py``, with the project's pytest
  configuration), which runs pytest's tests, unittest's ``TestCase`` classes
  and, through its doctest plugin, doctests. ``[tool.aye.doctest]`` reaches
  pytest as the options it stands for: ``modules = true`` as
  ``--doctest-modules``, each ``glob`` as ``--doctest-glob``, ``encoding``
  as ``-o doctest_encoding``; its other keys have no pytest spelling here.

When unittest is among several adapters, pytest is the referee for the ids
(it collects ``TestCase`` classes too) and for the outcomes only where they
agree with plain unittest's: recording runs unittest as well and fails on any
test it finds whose outcome pytest reports differently (an exception other
than an assertion is unittest's ``error`` and pytest's ``failed``), so such a
suite keeps to the cases both frameworks agree on.

A suite records one run with the paths ``expected.default_args`` gives it. An
example records two, from its directory with no paths, as its README runs it:
by default, and with ``AYE_EXAMPLE_FAIL=1``, which turns on its deliberate
failure. Every run is in a fresh copy of the project (no caches), with
``PYTEST_ADDOPTS`` and ``AYE_EXAMPLE_FAIL`` removed from the environment
unless the run sets them.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile

import tomllib

sys.dont_write_bytecode = True  # no __pycache__ beside the harness
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import expected
import expected_unittest

# The runs an example records: the environment each adds.
EXAMPLE_RUNS = ({}, {"AYE_EXAMPLE_FAIL": "1"})
# Removed from the environment of every run, so the developer's shell can't
# change what is recorded.
CLEARED = ("PYTEST_ADDOPTS", "AYE_EXAMPLE_FAIL")
SKIPPED = (".aye", ".pytest_cache", ".venv", "__pycache__", "expected.json")


def config(project: str) -> dict:
    path = os.path.join(project, "pyproject.toml")
    if not os.path.exists(path):
        return {}
    with open(path, "rb") as f:
        return tomllib.load(f)


def adapters(project: str) -> list[str] | None:
    """``[tool.aye] adapters``, or None when unset."""
    return config(project).get("tool", {}).get("aye", {}).get("adapters")


def referee(project: str) -> str:
    return "unittest" if adapters(project) == ["unittest"] else "pytest"


def doctest_options(project: str) -> list[str]:
    """``[tool.aye.doctest]`` as pytest's options."""
    section = config(project).get("tool", {}).get("aye", {}).get("doctest", {})
    options = []
    for key, value in section.items():
        if key == "modules" and value is True:
            options.append("--doctest-modules")
        elif key == "glob":
            globs = [value] if isinstance(value, str) else value
            options.extend(f"--doctest-glob={glob}" for glob in globs)
        elif key == "encoding":
            options.extend(["-o", f"doctest_encoding={value}"])
        else:
            sys.exit(f"{project}: [tool.aye.doctest] {key} = {value!r} has no pytest option here")
    return options


def environment(extra: dict[str, str]) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in CLEARED}
    env.update(extra)
    return env


def copy(project: str, into: str) -> str:
    # The real path: pytest and unittest report the paths of the directory
    # they run in as the operating system resolves it.
    target = os.path.join(os.path.realpath(into), os.path.basename(project))
    shutil.copytree(project, target, ignore=shutil.ignore_patterns(*SKIPPED))
    return target


def cross_check(project: str, pytest_run: dict, unittest_run: dict) -> None:
    """Fail unless pytest agrees with unittest on every test unittest finds."""
    disagree = []
    for test_id in unittest_run["collected"]:
        if test_id not in pytest_run["collected"]:
            disagree.append(f"{test_id}: unittest collects it, pytest doesn't")
            continue
        theirs = unittest_run["outcomes"].get(test_id)
        ours = pytest_run["outcomes"].get(test_id)
        if theirs != ours:
            disagree.append(f"{test_id}: unittest {theirs}, pytest {ours}")
    if disagree:
        lines = "\n  ".join(disagree)
        sys.exit(f"{project}: pytest can't referee these unittest tests:\n  {lines}")


def run(project: str, *, example: bool, env: dict[str, str]) -> dict:
    """One run of the project's referee, on a fresh copy."""
    with tempfile.TemporaryDirectory() as tmp:
        copied = copy(project, tmp)
        environ = environment(env)
        if referee(project) == "unittest":
            return expected_unittest.record(copied, env=environ)
        paths = [] if example else expected.default_args(copied)
        result = expected.record(copied, args=[*doctest_options(project), *paths], env=environ)
        if "unittest" in (adapters(project) or []):
            with tempfile.TemporaryDirectory() as other:
                again = copy(project, other)
                cross_check(project, result, expected_unittest.record(again, env=environ))
        return result


def record(project: str, *, example: bool) -> dict:
    project = os.path.abspath(project)
    recorded: dict = {"referee": referee(project)}
    if example:
        recorded["runs"] = [
            {"env": env, **run(project, example=True, env=env)} for env in EXAMPLE_RUNS
        ]
    else:
        recorded.update(run(project, example=False, env={}))
    return recorded


def main() -> None:
    args = sys.argv[1:]
    example = args[:1] == ["--example"]
    project, *rest = args[1:] if example else args
    text = json.dumps(record(project, example=example), indent=2, ensure_ascii=False) + "\n"
    if rest:
        with open(rest[0], "w") as f:
            f.write(text)
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()
