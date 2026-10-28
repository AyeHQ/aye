"""Runs the unittest adapter against scratch suites.

Every batch and refinement runs in a subprocess, as the worker runs them: fd
1 is kept for the reply and pointed at stderr, ``aye_engine`` and
``aye_engine_unittest`` are installed from source under ``<aye:…>`` file
names, and pytest is made unimportable first, since the executor must work
without it.
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
sys.path[:0] = [str(HERE.parent), str(ENGINE)]

MODULES = [
    ("aye_engine", str(ENGINE / "aye_engine.py")),
    ("aye_engine_unittest", str(HERE.parent / "aye_engine_unittest.py")),
]

DRIVER = r"""
import json, os, sys, types

# The executor imports only the standard library.
sys.modules["pytest"] = None
sys.modules["_pytest"] = None
request = json.loads(sys.argv[1])
reply_to = os.fdopen(os.dup(1), "w", encoding="utf-8")
os.dup2(2, 1)
if sys.path and sys.path[0] == "":
    sys.path[0] = os.getcwd()
for name, path in request["modules"]:
    module = types.ModuleType(name)
    sys.modules[name] = module
    with open(path, encoding="utf-8") as file:
        exec(compile(file.read(), f"<aye:{name}>", "exec"), module.__dict__)

import aye_engine

services = aye_engine.default_services()
seen = []
if request.get("watch_context"):
    context = services.get("context")
    import unittest
    original = unittest.TestCase.run
    def run(self, result=None):
        seen.append([context.test, list(context.lifetimes)])
        return original(self, result)
    unittest.TestCase.run = run
executor = aye_engine.load_entry("aye_engine_unittest:UnittestExecutor")(request["root"], services)
if "batches" in request:
    reply = []
    for batch in request["batches"]:
        sink = aye_engine.ListSink(batch=batch["id"])
        status = executor.run(aye_engine.Batch(batch), sink)
        reply.append({"status": status, "events": sink.events, "seen": seen})
else:
    reply = executor.refine(request["files"], request["context"])
out = {"reply": reply, "pytest_imported": sys.modules.get("pytest") is not None}
# As strict as the runner: JSON without NaN, in valid UTF-8.
reply_to.write(json.dumps(out, ensure_ascii=False, allow_nan=False))
reply_to.close()
"""


class Run:
    def __init__(self, reply, stderr):
        self.status = reply["status"]
        self.events = reply["events"]
        self.seen = reply.get("seen", [])
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

    def lifetimes(self):
        """``(+|-)lifetime`` in order, ``-`` with ``!`` when it failed."""
        out = []
        for event in self.events:
            if event["event"] == "lifetime_started":
                out.append("+" + event["lifetime"])
            elif event["event"] == "lifetime_finished":
                out.append("-" + event["lifetime"] + ("!" if event.get("failure") else ""))
        return out

    def failure(self, test_id, index=0):
        return self.finished[test_id]["failures"][index]


class Suite:
    """A scratch project: aye's root, with unittest's defaults unless the
    context says otherwise."""

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
            "start_directory": str(self.root),
            "top_level_directory": str(self.root),
            "pattern": "test*.py",
        }
        context.update(overrides)
        return context

    def _call(self, request, cwd=None):
        env = {k: v for k, v in os.environ.items() if not k.startswith("PYTEST")}
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        request = {"modules": MODULES, "root": str(self.root), **request}
        done = subprocess.run(
            [sys.executable, "-c", DRIVER, json.dumps(request)],
            cwd=self.root / (cwd or ""),
            env=env,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
        assert done.returncode == 0, done.stderr
        try:
            out = json.loads(done.stdout)
        except ValueError:
            pytest.fail(f"not a reply: {done.stdout!r}\n{done.stderr}")
        assert out["pytest_imported"] is False, "the executor imported pytest"
        return out["reply"], done.stderr

    def batch(self, ids, policy=None, context=None, id=3):
        return {
            "id": id,
            "adapter": "unittest",
            "ids": list(ids),
            "policy": policy or {},
            "context": self.context() if context is None else context,
        }

    def run(self, ids, policy=None, context=None, watch_context=False):
        return self.run_batches([self.batch(ids, policy, context)], watch_context)[0]

    def run_batches(self, batches, watch_context=False):
        """Run several batches in one worker, one after the other."""
        replies, stderr = self._call({"batches": batches, "watch_context": watch_context})
        return [Run(reply, stderr) for reply in replies]

    def refine(self, files, context=None, cwd=None):
        context = self.context() if context is None else context
        reply, _ = self._call({"files": list(files), "context": context}, cwd)
        return reply

    def unittest(self, *args):
        """``python -m unittest`` itself, for comparison."""
        return subprocess.run(
            [sys.executable, "-m", "unittest", *args],
            cwd=self.root,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )


@pytest.fixture
def suite(tmp_path):
    return Suite(tmp_path)
