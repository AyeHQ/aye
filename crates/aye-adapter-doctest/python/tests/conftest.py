"""Runs the doctest adapter against scratch suites.

Every batch and refinement runs in a subprocess, as in a worker: it keeps fd
1 for its reply and points fd 1 at stderr, installs ``aye_engine`` and
``aye_engine_doctest`` from source under ``<aye:…>`` file names, and hands the
executor a batch or a refinement. With ``no_pytest``, the subprocess can't
import pytest, as in an environment without it.
"""

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
ENGINE = HERE.parents[2] / "aye-engine" / "python"

MODULES = [
    ("aye_engine", str(ENGINE / "aye_engine.py")),
    ("aye_engine_doctest", str(HERE.parent / "aye_engine_doctest.py")),
]

DRIVER = r"""
import json, os, sys, types

request = json.loads(sys.argv[1])
reply_to = os.fdopen(os.dup(1), "w", encoding="utf-8")
os.dup2(2, 1)
# As the worker does: the working directory first on sys.path.
if sys.path and sys.path[0] == "":
    sys.path[0] = os.getcwd()

if request.get("no_pytest"):
    class NoPytest:
        def find_spec(self, name, path=None, target=None):
            if name.split(".")[0] in ("pytest", "_pytest"):
                raise ModuleNotFoundError(f"No module named {name!r}")
            return None

    sys.meta_path.insert(0, NoPytest())

for name, path in request["modules"]:
    module = types.ModuleType(name)
    sys.modules[name] = module
    with open(path, encoding="utf-8") as file:
        exec(compile(file.read(), f"<aye:{name}>", "exec"), module.__dict__)

import aye_engine

SERVICES = aye_engine.default_services()
executor = aye_engine.load_entry("aye_engine_doctest:DoctestExecutor")(request["root"], SERVICES)
if "batches" in request:
    reply = []
    for batch in request["batches"]:
        sink = aye_engine.ListSink(batch=batch["id"])
        try:
            status = executor.run(aye_engine.Batch(batch), sink)
        except BaseException as exc:
            status = f"raised {type(exc).__name__}: {exc}"
        reply.append(
            {"status": status, "events": sink.events, "pytest_imported": "pytest" in sys.modules}
        )
else:
    reply = executor.refine(request["files"], request["context"])
    reply["pytest_imported"] = "pytest" in sys.modules
# As strict as the runner: JSON without NaN, in valid UTF-8.
reply_to.write(json.dumps(reply, ensure_ascii=False, allow_nan=False))
reply_to.close()
"""

# Variables that would change what the subprocesses do.
SCRUBBED = (
    "PYTEST_ADDOPTS",
    "PYTEST_PLUGINS",
    "PYTEST_CURRENT_TEST",
    "PYTEST_VERSION",
    "PYTHONOPTIMIZE",
)


class Run:
    def __init__(self, reply, stderr):
        self.status = reply["status"]
        self.events = reply["events"]
        self.pytest_imported = reply["pytest_imported"]
        self.stderr = stderr

    def of(self, kind):
        return [e for e in self.events if e["event"] == kind]

    @property
    def started(self):
        return [e["id"] for e in self.of("test_started")]

    @property
    def finished(self):
        return {e["id"]: e for e in self.of("test_finished")}

    @property
    def outcomes(self):
        return {e["id"]: e["outcome"] for e in self.of("test_finished")}

    @property
    def diagnostics(self):
        return self.of("diagnostic")

    def subs(self, test_id):
        return [(e["name"], e["outcome"]) for e in self.of("sub_outcome") if e["id"] == test_id]

    def failure(self, test_id, index=0):
        return self.finished[test_id]["failures"][index]


class Suite:
    """A scratch project: aye's root."""

    def __init__(self, root):
        self.root = root

    def write(self, files):
        for name, source in files.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(source).lstrip("\n"), encoding="utf-8")
        return self

    def context(self, **overrides):
        context = {
            "rootdir": str(self.root),
            "optionflags": ["ELLIPSIS"],
            "encoding": "utf-8",
            "import_mode": "prepend",
            "report": "udiff",
        }
        context.update(overrides)
        return context

    def env(self, extra=None):
        env = {k: v for k, v in os.environ.items() if k not in SCRUBBED}
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        return {**env, **(extra or {})}

    def _call(self, request, env=None, python=None):
        request = {"modules": MODULES, "root": str(self.root), **request}
        done = subprocess.run(
            [python or sys.executable, "-c", DRIVER, json.dumps(request)],
            cwd=self.root,
            env=self.env(env),
            capture_output=True,
            text=True,
            timeout=600,
            check=False,
        )
        assert done.returncode == 0, done.stderr
        try:
            return json.loads(done.stdout), done.stderr
        except ValueError:
            pytest.fail(f"not a reply: {done.stdout!r}\n{done.stderr}")

    def batch(self, ids, policy=None, context=None, id=3):
        return {
            "id": id,
            "adapter": "doctest",
            "ids": list(ids),
            "policy": policy or {},
            "context": self.context() if context is None else context,
        }

    def run(self, ids, policy=None, context=None, env=None, no_pytest=False, python=None):
        request = {"batches": [self.batch(ids, policy, context)], "no_pytest": no_pytest}
        replies, stderr = self._call(request, env, python)
        return Run(replies[0], stderr)

    def refine(self, files, context=None, env=None, no_pytest=False, python=None):
        context = self.context() if context is None else context
        request = {"files": list(files), "context": context, "no_pytest": no_pytest}
        reply, _ = self._call(request, env, python)
        return reply

    def pytest(self, *args, env=None):
        """pytest itself, run on the suite, for comparison."""
        return subprocess.run(
            [sys.executable, "-m", "pytest", "-p", "no:cacheprovider", *args],
            cwd=self.root,
            env=self.env(env),
            capture_output=True,
            text=True,
            timeout=600,
            check=False,
        )

    def pytest_ids(self, *args):
        """The node ids pytest collects, in its order (``--co -q``)."""
        done = self.pytest("--co", "-q", *args)
        ids = []
        for line in done.stdout.splitlines():
            if not line.strip():
                break
            ids.append(line.strip())
        return ids

    def pytest_outcomes(self, *args):
        """pytest's outcome per node id, as aye names them."""
        plugin = self.root.parent / "aye_doctest_outcomes.py"
        plugin.write_text(OUTCOMES_PLUGIN, encoding="utf-8")
        out = self.root.parent / "outcomes.json"
        self.pytest(
            "-p",
            "aye_doctest_outcomes",
            *args,
            env={"PYTHONPATH": str(plugin.parent), "AYE_OUTCOMES": str(out)},
        )
        return json.loads(out.read_text(encoding="utf-8"))


OUTCOMES_PLUGIN = r"""
import json, os

SEVERITY = ["passed", "skipped", "expected_failure", "unexpected_pass", "failed", "error"]
OUTCOMES = {}


def pytest_runtest_logreport(report):
    if report.when == "call":
        if hasattr(report, "wasxfail"):
            outcome = "expected_failure" if report.skipped else "unexpected_pass"
        else:
            outcome = report.outcome
    elif report.failed:
        outcome = "error"
    elif report.skipped:
        outcome = "skipped"
    else:
        outcome = "passed"
    old = OUTCOMES.get(report.nodeid, "passed")
    OUTCOMES[report.nodeid] = max(old, outcome, key=SEVERITY.index)


def pytest_sessionfinish():
    with open(os.environ["AYE_OUTCOMES"], "w") as f:
        json.dump(OUTCOMES, f)
"""


@pytest.fixture
def suite(tmp_path):
    root = tmp_path / "project"
    root.mkdir()
    return Suite(root)
