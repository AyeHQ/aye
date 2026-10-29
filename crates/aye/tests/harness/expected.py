"""Record what plain pytest does on an example suite, in aye's vocabulary.

    python expected.py <suite-dir> [<output.json>]

The pytest referee: ``record.py`` picks each suite's referee and calls
:func:`record`. It runs pytest twice in subprocesses (collect only, then a
full run) and gives {"collected": [ids], "outcomes": {id: outcome},
"exit_code": status}: ids relative to the suite directory (aye's project root
for the suite) in pytest's order, one outcome per test, the worst of its
phases, as aye reports them, and the full run's exit status. Subtests don't
change their test's outcome, except that a failed one of pytest's own fails
it, as pytest's terminal report does. The comparison harness diffs aye's
results against these files.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile

PLUGIN = r"""
import json, os, pytest

ROOT = os.environ["AYE_SUITE_ROOT"]
OUT = os.environ["AYE_SUITE_OUT"]
SEVERITY = ["passed", "skipped", "expected_failure", "unexpected_pass", "failed", "error"]


def aye_id(item_path, nodeid):
    rel = os.path.relpath(str(item_path), ROOT).replace(os.sep, "/")
    _, sep, rest = nodeid.partition("::")
    return rel + (sep + rest if sep else "")


def is_subtest(report):
    # A subtest report carries its context (pytest 9's subtests, the
    # pytest-subtests plugin), and the nodeid of the test holding it.
    context = getattr(report, "context", None)
    return context is not None and hasattr(context, "kwargs") and hasattr(context, "msg")


def is_builtin_subtest(report):
    return type(report).__module__ == "_pytest.subtests"


class Recorder:
    def __init__(self):
        self.collected = []
        self.paths = {}
        self.outcomes = {}

    def pytest_collection_modifyitems(self, items):
        for item in items:
            self.paths[item.nodeid] = item.path
            self.collected.append(aye_id(item.path, item.nodeid))

    def pytest_runtest_logreport(self, report):
        path = self.paths.get(report.nodeid)
        if path is None:
            return
        if is_subtest(report) and not (report.failed and is_builtin_subtest(report)):
            # A subtest's result is its own; the test keeps its outcome,
            # except that pytest's own subtests fail the test holding a
            # failed one, as its terminal report does.
            return
        key = aye_id(path, report.nodeid)
        if report.when == "call":
            if hasattr(report, "wasxfail"):
                outcome = "expected_failure" if report.skipped else "unexpected_pass"
            else:
                outcome = {"passed": "passed", "failed": "failed", "skipped": "skipped"}[report.outcome]
        elif report.failed:
            outcome = "error"
        elif report.skipped:
            outcome = "expected_failure" if hasattr(report, "wasxfail") else "skipped"
        else:
            outcome = "passed"
        old = self.outcomes.get(key, "passed")
        self.outcomes[key] = max(old, outcome, key=SEVERITY.index)

    def pytest_sessionfinish(self):
        with open(OUT, "w") as f:
            json.dump({"collected": self.collected, "outcomes": self.outcomes}, f)


def pytest_configure(config):
    config.pluginmanager.register(Recorder(), "aye-suite-recorder")
"""


def pytest(project: str, args: list[str], plugin_dir: str, out: str, env: dict) -> int:
    env = dict(env, AYE_SUITE_ROOT=project, AYE_SUITE_OUT=out, PYTHONPATH=plugin_dir)
    cmd = [
        sys.executable,
        "-m",
        "pytest",
        "-p",
        "aye_suite_recorder",
        "-p",
        "no:cacheprovider",
        "-q",
        *args,
    ]
    done = subprocess.run(cmd, cwd=project, env=env, capture_output=True, text=True, check=False)
    if not os.path.exists(out):
        sys.exit(
            f"pytest {' '.join(args)} in {project} exited {done.returncode}:\n{done.stdout}{done.stderr}"
        )
    return done.returncode


def default_args(project: str) -> list[str]:
    """The paths a suite's run gets: none when it has a ``pyproject.toml``
    and a ``tests`` directory (``testpaths`` or the current directory apply),
    else its top-level directories. ``compare.rs`` gives aye the same."""
    targets = [d for d in sorted(os.listdir(project)) if os.path.isdir(os.path.join(project, d))]
    if os.path.exists(os.path.join(project, "pyproject.toml")) and "tests" in targets:
        return []
    return targets


def record(project: str, args: list[str] | None = None, env: dict | None = None) -> dict:
    """What plain pytest does on ``project``, run from it with ``args``
    (default: :func:`default_args`) in ``env`` (default: this process's):
    {"collected": [ids], "outcomes": {id: outcome}, "exit_code": status},
    the status being the full run's."""
    project = os.path.realpath(project)
    args = default_args(project) if args is None else args
    env = dict(os.environ) if env is None else env
    with tempfile.TemporaryDirectory() as tmp:
        with open(os.path.join(tmp, "aye_suite_recorder.py"), "w") as f:
            f.write(PLUGIN)
        collect_out = os.path.join(tmp, "collect.json")
        run_out = os.path.join(tmp, "run.json")
        pytest(project, ["--collect-only", *args], tmp, collect_out, env)
        status = pytest(project, args, tmp, run_out, env)
        with open(collect_out) as f:
            collected = json.load(f)["collected"]
        with open(run_out) as f:
            outcomes = json.load(f)["outcomes"]
    return {"collected": collected, "outcomes": dict(sorted(outcomes.items())), "exit_code": status}


def main() -> None:
    project, *rest = sys.argv[1:]
    result = {"referee": "pytest", **record(project)}
    text = json.dumps(result, indent=2, ensure_ascii=False) + "\n"
    if rest:
        with open(rest[0], "w") as f:
            f.write(text)
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()
