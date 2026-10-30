"""The pytest front-end and its glue (``native/aye_pytest_frontend.py``,
``native/aye_pytest_native.py``), against plain pytest.

Everything that needs aye's ``pytest`` module runs in a subprocess that
installs the modules as the worker does (their source, under ``<aye:NAME>``
file names, in the worker's order: ``aye_engine``, ``aye_model``,
``aye_rewrite``, ``aye_native_engine``, the shim as ``pytest``, the front-end,
the glue), with ``python -S``: no site-packages, so no real pytest can be
imported there. The referee is plain pytest: the comparison harness's
recorder (``crates/aye/tests/harness/expected.py``), and what it collects,
read by plugins and conftests the tests give it.
"""

import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
PYTHON = HERE.parents[1]
CRATES = HERE.parents[3]

# The comparison harness's pytest recorder (``harness/expected.py``).
_spec = importlib.util.spec_from_file_location(
    "aye_harness_expected", CRATES / "aye" / "tests" / "harness" / "expected.py"
)
referee = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(referee)

MODULES = [
    ("aye_engine", CRATES / "aye-engine" / "python" / "aye_engine.py"),
    ("aye_model", CRATES / "aye-engine" / "python" / "aye_model.py"),
    ("aye_rewrite", CRATES / "aye-engine" / "python" / "aye_rewrite.py"),
    ("aye_native_engine", CRATES / "aye-engine" / "python" / "aye_native_engine.py"),
    ("pytest", PYTHON / "native" / "aye_pytest_shim.py"),
    ("aye_pytest_frontend", PYTHON / "native" / "aye_pytest_frontend.py"),
    ("aye_pytest_native", PYTHON / "native" / "aye_pytest_native.py"),
]

# The worker's bootstrap: fd 1 kept for the reply and pointed at stderr, the
# modules installed from source as ``<aye:NAME>``, then one request.
DRIVER = r"""
import json, linecache, os, sys, types

request = json.loads(sys.argv[1])
reply_to = os.fdopen(os.dup(1), "w", encoding="utf-8")
os.dup2(2, 1)
for name, path in request["modules"]:
    with open(path, encoding="utf-8") as file:
        source = file.read()
    filename = f"<aye:{name}>"
    module = types.ModuleType(name)
    sys.modules[name] = module
    linecache.cache[filename] = (len(source), None, source.splitlines(True), filename)
    exec(compile(source, filename, "exec", dont_inherit=True), module.__dict__)

import aye_engine

executor = aye_engine.load_entry("aye_pytest_native:NativeExecutor")(
    request["root"], aye_engine.default_services()
)


def describe(frontend, case):
    def chain(node):
        return [[n.id, n.kind] for n in node.chain()]

    visible = {
        name: [d.baseid for d in case.node.visible.get(name, ())]
        for name in case.closure
    }
    return {
        "id": case.id,
        "kind": case.kind,
        "closure": list(case.closure),
        "argnames": list(case.argnames),
        "call_params": {k: repr(v) for k, v in case.call_params.items()},
        "resource_params": {
            k: [i, repr(v), part] for k, (i, v, part) in case.resource_params.items()
        },
        "param_scopes": {k: v.name.lower() for k, v in case.param_scopes.items()},
        "tags": [t.name for t in case.tags],
        "nodes": chain(case.node),
        "func": getattr(case.func, "__qualname__", repr(case.func)),
        "cls": case.cls.__name__ if case.cls else None,
        "location": list(case.location),
        "visible": visible,
        "fixturenames": list(case.extra["item"].fixturenames),
    }


if request["mode"] == "refine":
    reply = executor.refine(request["files"], request["context"])
elif request["mode"] == "model":
    frontend = executor.frontend(request["context"])
    cases, diagnostics = frontend.collect(request["files"])
    reply = {
        "cases": [describe(frontend, case) for case in cases],
        "diagnostics": diagnostics,
        "policy": {
            "xfail_strict": frontend.policy.xfail_strict,
            "namespace": sorted(frontend.policy.condition_namespace),
        },
        "hooks": {
            phase: len(getattr(frontend.hooks, phase)) for phase in ("setup", "call", "teardown")
        },
        "rewrite": {p: frontend.should_rewrite(os.path.join(request["root"], p)) for p in request.get("rewrite", [])},
    }
    frontend.finish(0)
elif request["mode"] == "session":
    # One worker's life: refinements, batches and the close, in order.
    reply = []
    for step in request["steps"]:
        if step["op"] == "refine":
            kwargs = {"digests_only": True} if step.get("digests_only") else {}
            reply.append(executor.refine(step["files"], step["context"], **kwargs))
        elif step["op"] == "run":
            sink = aye_engine.ListSink(batch=step["batch"]["id"])
            status = executor.run(aye_engine.Batch(step["batch"]), sink)
            reply.append({"status": status, "events": sink.events})
        else:
            executor.close()
            reply.append(None)
else:
    reply = []
    for batch in request["batches"]:
        sink = aye_engine.ListSink(batch=batch["id"])
        status = executor.run(aye_engine.Batch(batch), sink)
        reply.append({"status": status, "events": sink.events})
reply_to.write(json.dumps(reply, ensure_ascii=False, allow_nan=False, default=repr))
reply_to.close()
"""

# Variables that would change what pytest, or the front-end, does.
SCRUBBED = ("PYTEST_ADDOPTS", "PYTEST_PLUGINS", "PYTEST_CURRENT_TEST", "PYTEST_VERSION")


class Project:
    """A scratch pytest project: aye's root, run natively and by pytest."""

    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        # Where pytest's and aye's numbered temporary directories go: not
        # the ``pytest-of-<user>`` this session uses, which their rotation
        # would remove.
        self.temproot = self.root.parent / "temproot"
        self.temproot.mkdir(parents=True, exist_ok=True)

    def environ(self, env=None):
        scrubbed = {k: v for k, v in os.environ.items() if k not in SCRUBBED}
        return {**scrubbed, "PYTEST_DEBUG_TEMPROOT": str(self.temproot), **(env or {})}

    def write(self, files: dict) -> "Project":
        for name, source in files.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(source).lstrip("\n"), encoding="utf-8")
        return self

    def context(self, **overrides):
        context = {"rootdir": str(self.root), "roots": [str(self.root)]}
        if (self.root / "pytest.ini").exists():
            context["inifile"] = str(self.root / "pytest.ini")
        context.update(overrides)
        return context

    def _call(self, request: dict, env=None):
        env = self.environ(env)
        request = {"modules": [[n, str(p)] for n, p in MODULES], "root": str(self.root), **request}
        done = subprocess.run(
            [sys.executable, "-S", "-c", DRIVER, json.dumps(request)],
            cwd=self.root,
            env=env,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
        assert done.returncode == 0, done.stderr
        self.stderr = done.stderr
        return json.loads(done.stdout)

    def refine(self, files, context=None, env=None):
        context = self.context() if context is None else context
        return self._call({"mode": "refine", "files": list(files), "context": context}, env)

    def model(self, files, context=None, rewrite=()):
        context = self.context() if context is None else context
        return self._call(
            {"mode": "model", "files": list(files), "context": context, "rewrite": list(rewrite)}
        )

    def run(self, ids, context=None, policy=None, env=None):
        batch = {
            "id": 1,
            "adapter": "pytest",
            "ids": list(ids),
            "policy": policy or {},
            "context": self.context() if context is None else context,
        }
        return Run(self._call({"mode": "run", "batches": [batch]}, env)[0])

    def session(self, *steps, env=None):
        """One worker's life: ``("refine", files)``, ``("digests", files)``,
        ``("run", ids)``, ``("last", ids)`` (the batch that ends the
        session), ``("end",)`` (a batch with no ids that only ends it) and
        ``("close",)`` steps, in order; each step's reply (a ``Run`` for a
        batch)."""
        context = self.context()
        request = []
        for number, (op, *args) in enumerate(steps):
            if op in ("refine", "digests"):
                request.append(
                    {
                        "op": "refine",
                        "files": list(args[0]),
                        "context": context,
                        "digests_only": op == "digests",
                    }
                )
            elif op in ("run", "last", "end"):
                batch = {
                    "id": number,
                    "adapter": "pytest",
                    "ids": list(args[0]) if args else [],
                    "policy": {},
                    "context": context,
                }
                if op != "run":
                    batch["ends_session"] = True
                request.append({"op": "run", "batch": batch})
            else:
                request.append({"op": "close"})
        replies = self._call({"mode": "session", "steps": request}, env)
        return [Run(r) if r is not None and "events" in r else r for r in replies]

    def files(self):
        """The project's test files pytest collects, relative, sorted."""
        return sorted(
            str(p.relative_to(self.root)).replace(os.sep, "/") for p in self.root.rglob("test_*.py")
        )

    def pytest_ids(self, *args, env=None):
        """The ids ``pytest --collect-only -q`` prints: its order once every
        ``pytest_collection_modifyitems`` hook ran (the recorder records the
        order before the start-up conftests' hooks)."""
        done = subprocess.run(
            [sys.executable, "-m", "pytest", "-p", "no:cacheprovider", "--co", "-q", *args],
            cwd=self.root,
            env=self.environ(env),
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
        found = []
        for line in done.stdout.splitlines():
            if not line.strip():
                break
            found.append(line.strip())
        return found

    def pytest(self, *args, env=None):
        """What plain pytest does: collected ids, outcomes, exit status."""
        return referee.record(str(self.root), list(args), self.environ(env))


class Run:
    def __init__(self, reply):
        self.status = reply["status"]
        self.events = reply["events"]

    def of(self, kind):
        return [e for e in self.events if e["event"] == kind]

    @property
    def started(self):
        return [e["id"] for e in self.of("test_started")]

    @property
    def outcomes(self):
        return {e["id"]: e["outcome"] for e in self.of("test_finished")}

    @property
    def finished(self):
        return {e["id"]: e for e in self.of("test_finished")}

    @property
    def diagnostics(self):
        return [(e["severity"], e["code"], e["path"]) for e in self.of("diagnostic")]


@pytest.fixture
def project(tmp_path):
    return Project(tmp_path / "project").write({"pytest.ini": "[pytest]\n"})


def ids(refined):
    return [case["id"] for case in refined["cases"]]


def same_as_pytest(project, files=None, context=None, env=None, args=()):
    """Refine, then run every refined id natively: the same ids in the same
    order, the same outcome per id and the same status as plain pytest."""
    expected = project.pytest(*args, env=env)
    files = files if files is not None else project.files()
    refined = project.refine(files, context, env)
    assert ids(refined) == expected["collected"]
    run = project.run(ids(refined), context, env=env)
    assert run.outcomes == expected["outcomes"]
    assert run.status == expected["exit_code"]
    return refined, run


# ------------------------------------------------------------------ ids


IDS = {
    "conftest.py": """
        import pytest

        def pytest_generate_tests(metafunc):
            if "flavour" in metafunc.fixturenames:
                metafunc.parametrize("flavour", ["salt", "pepper"])

        @pytest.fixture(params=[1, 2], ids=["one", "two"])
        def numbered(request):
            return request.param
    """,
    "tests/test_ids.py": """
        import enum
        import re

        import pytest

        class Color(enum.Enum):
            RED = 1

        @pytest.mark.parametrize("x", [0, 1])
        @pytest.mark.parametrize("y", [2, 3])
        def test_stacked(x, y):
            pass

        @pytest.mark.parametrize(
            "value",
            ["a", "a", "a0", "b1", "b1", "", "", "é", b"\\xc3", "\\n", 1.5, None, True,
             (1, 2), [1], {1: 2}, object, str.lower, Color.RED, re.compile("x+"), 2j, -0.0],
        )
        def test_values(value):
            pass

        @pytest.mark.parametrize(
            "x", [1, pytest.param(2, id="x"), 3], ids=["first", None, "third"]
        )
        def test_ids_list(x):
            pass

        @pytest.mark.parametrize("n", range(3), ids=lambda n: None if n == 1 else f"n{n}")
        def test_idfn(n):
            pass

        @pytest.mark.parametrize("x", [pytest.param(1, marks=pytest.mark.skip), 2])
        def test_param_marks(x):
            pass

        @pytest.mark.parametrize("x", [])
        def test_empty(x):
            pass

        @pytest.mark.parametrize("numbered", [10], indirect=True)
        def test_indirect(numbered):
            pass

        def test_fixture_params(numbered):
            pass

        def test_generated(flavour, numbered):
            pass

        @pytest.mark.parametrize("x", ["M"])
        class TestCls:
            @pytest.mark.parametrize("y", ["F"])
            def test_order(self, x, y):
                pass

            def pytest_generate_tests(self, metafunc):
                if "z" in metafunc.fixturenames:
                    metafunc.parametrize("z", ["c1"])

            def test_cls_hook(self, x, z):
                pass

        def pytest_generate_tests(metafunc):
            if "w" in metafunc.fixturenames:
                metafunc.parametrize("w", ["m1"])

        def test_module_hook(w, flavour):
            pass
    """,
    "tests/test_shapes.py": """
        import unittest

        import pytest

        pytestmark = pytest.mark.parametrize("flav", ["v", "c"])

        class Base:
            def test_base(self, flav):
                pass

        class TestChild(Base):
            def test_child(self, flav):
                pass

            def test_over(self, flav):
                pass

            class TestNested:
                def test_n(self, flav):
                    pass

            @staticmethod
            def test_static(flav):
                pass

        class TestInit:
            def __init__(self):
                pass

            def test_never(self):
                pass

        class TestOff:
            __test__ = False

            def test_never(self):
                pass

        def helper(flav):
            pass

        test_alias = helper

        class Zeta(unittest.TestCase):
            def test_b(self):
                pass

            def test_a(self):
                pass
    """,
    "tests/pkg/__init__.py": "",
    "tests/pkg/test_deep.py": """
        def test_deep():
            pass
    """,
}


def test_ids_and_order_are_pytests(project):
    project.write(IDS)
    expected = project.pytest("--collect-only")
    refined = project.refine(project.files())
    assert ids(refined) == expected["collected"]
    assert refined["diagnostics"] == []


# Plain pytest's items as refined cases: what the session-driving executor
# (which ran each batch in a real pytest session) reported for them, from
# what plain pytest collects. Run as ``python -c`` in the project's root,
# with the output file as its argument.
REFINED_BY_PYTEST = r"""
import json, math, os, sys

import pytest
from _pytest.mark import KeywordMatcher

ROOT = os.getcwd()


def timeout(item):
    marker = item.get_closest_marker("timeout")
    if marker is None:
        return None
    value = marker.args[0] if marker.args else marker.kwargs.get("timeout")
    if isinstance(value, bool):
        return None
    try:
        value = float(value)
    except (TypeError, ValueError, OverflowError):
        return None
    return value if math.isfinite(value) and value > 0 else None


class Refined:
    cases = None

    def pytest_collection_finish(self, session):
        self.cases = []
        for item in session.items:
            file, line, _ = item.location
            path = os.path.relpath(os.path.join(str(session.config.rootpath), file), ROOT)
            span = {"path": path.replace(os.sep, "/"), "start": 0, "end": 0}
            if line is not None:
                span["line"] = line + 1
            case = {"id": item.nodeid, "adapter": "pytest", "span": span}
            tags = list(dict.fromkeys(marker.name for marker in item.iter_markers()))
            if tags:
                case["tags"] = tags
            names = sorted(KeywordMatcher.from_item(item)._names)
            if names:
                case["keywords"] = names
            found = timeout(item)
            if found is not None:
                case["timeout"] = found
            case["certainty"] = "refined"
            self.cases.append(case)


plugin = Refined()
pytest.main(["-p", "no:cacheprovider", "--collect-only", "-q"], plugins=[plugin])
with open(sys.argv[1], "w", encoding="utf-8") as out:
    json.dump({"cases": plugin.cases, "diagnostics": []}, out)
"""


def test_refined_cases_are_plain_pytests(project, tmp_path):
    """Field for field, plain pytest's items as refined cases (the span from
    ``item.location``, the marks' names as tags, the names ``-k`` matches,
    the ``timeout`` mark), but the affinity, which says only what the cases
    must share (see ``test_affinity_*``), and the digests."""
    project.write(IDS)
    out = tmp_path / "refined.json"
    subprocess.run(
        [sys.executable, "-c", REFINED_BY_PYTEST, str(out)],
        cwd=project.root,
        env=project.environ(),
        capture_output=True,
        timeout=120,
        check=True,
    )
    expected = json.loads(out.read_text(encoding="utf-8"))
    files = project.files()
    native = project.refine(files)
    assert set(native.pop("digests")) == set(files)
    for case in native["cases"]:
        case.pop("affinity")
    assert native == expected


# ------------------------------------------------------------------ outcomes


OUTCOMES = {
    "conftest.py": """
        import pytest

        EVENTS = []

        @pytest.fixture(scope="session")
        def events():
            return EVENTS

        @pytest.fixture
        def base():
            return "root"

        @pytest.fixture(scope="module")
        def module_resource(events):
            events.append("module setup")
            yield "m"
            events.append("module teardown")

        @pytest.fixture(scope="module", params=["m1", "m2"])
        def moduleparam(request, events):
            events.append(f"setup {request.param}")
            yield request.param
            events.append(f"teardown {request.param}")

        @pytest.fixture
        def broken():
            raise RuntimeError("database is down")

        @pytest.fixture
        def bad_teardown():
            yield
            raise OSError("disk full")
    """,
    "tests/conftest.py": """
        import pytest

        @pytest.fixture
        def base(base):
            return base + "+sub"
    """,
    "tests/test_scopes.py": """
        import pytest

        @pytest.fixture
        def base(base):
            return base + "+mod"

        def setup_module(module):
            module.STATE = ["setup_module"]

        def teardown_module(module):
            assert module.STATE

        def setup_function(function):
            STATE.append(function.__name__)

        class TestOverride:
            @pytest.fixture
            def base(self, base):
                return base + "+cls"

            def setup_method(self, method):
                self.method = method.__name__

            def test_chain(self, base):
                assert base == "root+sub+mod+cls"
                assert self.method == "test_chain"

        def test_module_level(base):
            assert base == "root+sub+mod"
            assert STATE[-1] == "test_module_level"

        def test_a(moduleparam, events):
            assert moduleparam in ("m1", "m2")

        def test_b(moduleparam, events):
            assert events[-1] == f"setup {moduleparam}"

        def test_unparametrized(module_resource):
            assert module_resource == "m"

        def test_events_order(events):
            assert events == ["setup m1", "teardown m1", "setup m2", "module setup"]
    """,
    "tests/test_outcomes.py": """
        import sys
        import unittest

        import pytest

        def test_fails():
            assert [1, 2, 3] == [1, 2, 4]

        def test_raises():
            raise ValueError("boom")

        def test_setup_error(broken):
            pass

        def test_teardown_error(bad_teardown):
            pass

        @pytest.mark.skip(reason="not today")
        def test_skip():
            pass

        @pytest.mark.skipif("sys.platform != 'nowhere'", reason="everywhere")
        def test_skipif_string():
            pass

        def test_skip_inside():
            pytest.skip("at runtime")

        @pytest.mark.xfail(reason="known")
        def test_xfail():
            assert False

        @pytest.mark.xfail(reason="fixed")
        def test_xpass():
            pass

        @pytest.mark.xfail(strict=True, reason="strict")
        def test_xpass_strict():
            pass

        @pytest.mark.xfail(raises=KeyError)
        def test_xfail_wrong_exception():
            raise ValueError

        @pytest.mark.xfail(run=False)
        def test_xfail_not_run():
            raise SystemExit

        def test_fail_helper():
            pytest.fail("explicit")

        class TestCase(unittest.TestCase):
            @classmethod
            def setUpClass(cls):
                cls.opened = True

            def test_ok(self):
                self.assertTrue(self.opened)

            def test_fails(self):
                self.assertEqual(1, 2)

            @unittest.expectedFailure
            def test_expected(self):
                self.assertEqual(1, 2)

            @unittest.skip("later")
            def test_skipped(self):
                pass

        class BrokenSetup(unittest.TestCase):
            @classmethod
            def setUpClass(cls):
                raise RuntimeError("no database")

            def test_one(self):
                pass

        def test_tmp_path_and_capsys(tmp_path, capsys, monkeypatch):
            monkeypatch.setenv("AYE_FRONTEND_TEST", "1")
            (tmp_path / "f").write_text("x")
            print("hello")
            assert capsys.readouterr().out == "hello\\n"
    """,
}


def test_outcomes_are_pytests(project):
    project.write(OUTCOMES)
    _, run = same_as_pytest(project)
    failure = run.finished["tests/test_outcomes.py::test_fails"]["failures"][0]
    assert failure["assertion"]["op"] == "=="
    assert failure["location"] == {"path": "tests/test_outcomes.py", "line": 7}


def test_example_project_runs_as_under_pytest(tmp_path):
    """``examples/pytest``: every scope of fixture, parametrize with ids, a
    class, marks, skip and xfail, ``tmp_path``/``monkeypatch``/``capsys``,
    and a rich dict diff when its deliberate failure is on."""
    root = tmp_path / "pytest"
    shutil.copytree(
        CRATES.parent / "examples" / "pytest",
        root,
        ignore=shutil.ignore_patterns("__pycache__", ".pytest_cache", ".venv"),
    )
    example = Project(root)
    context = example.context(
        inifile=str(example.root / "pyproject.toml"), roots=[str(example.root / "tests")]
    )
    _, run = same_as_pytest(
        example, files=example.files(), context=context, env={"AYE_EXAMPLE_FAIL": "1"}
    )
    failure = run.finished["tests/test_cart.py::test_summary_after_coupon"]["failures"][0]
    assert "Differing items:" in failure["assertion"]["diff"]
    # pytest's crash line (``assertrepr_compare``'s summary, each side cut
    # to fit), as the pytest adapter reports it: the same failure signature.
    assert failure["message"] == (
        "assert {'subtotal': ...cimal('9.44')} == {'subtotal': ...cimal('9.45')}"
    )


def test_a_plugin_pytest_would_load_is_named_once_by_refinement(project, tmp_path):
    """An installed ``pytest11`` plugin aye doesn't load is a warning from
    refinement, which every native run does; batches don't repeat it."""
    site = tmp_path / "site"
    info = site / "fakeplugin-1.0.dist-info"
    info.mkdir(parents=True)
    (info / "METADATA").write_text("Metadata-Version: 2.1\nName: fakeplugin\nVersion: 1.0\n")
    (info / "entry_points.txt").write_text("[pytest11]\nfake = fake_plugin\n")
    project.write({"tests/test_a.py": "def test_a():\n    pass\n"})
    env = {"PYTHONPATH": str(site)}
    refined = project.refine(["tests/test_a.py"], env=env)
    warnings = [d for d in refined["diagnostics"] if d["code"] == "plugin-not-loaded"]
    assert len(warnings) == 1
    assert "'fake' (fakeplugin)" in warnings[0]["message"]
    run = project.run(ids(refined), env=env)
    assert run.outcomes == {"tests/test_a.py::test_a": "passed"}
    assert run.diagnostics == []


def test_an_option_nothing_defines_is_a_usage_error(project, tmp_path):
    """pytest stops with a usage error (status 4) on an option that no
    builtin, conftest or plugin defines, and so does aye; one a ``-p``
    plugin defines is that plugin's. With a plugin pytest would load by
    itself and aye doesn't, the option may be that plugin's: a warning, and
    aye ignores it."""
    off = {"PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1"}
    project.write(
        {
            "pytest.ini": "[pytest]\naddopts = --bogus-opt\n",
            "tests/test_a.py": "def test_a(pytestconfig):\n    assert pytestconfig.getoption('bogus_opt')\n",
        }
    )
    done = subprocess.run(
        [sys.executable, "-m", "pytest", "-p", "no:cacheprovider", "-q"],
        cwd=project.root,
        env=project.environ(off),
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    assert done.returncode == 4
    assert "unrecognized arguments: --bogus-opt" in done.stderr
    refined = project.refine(project.files(), env=off)
    assert refined["cases"] == []
    (diagnostic,) = refined["diagnostics"]
    assert (diagnostic["severity"], diagnostic["code"]) == ("error", "usage-error")
    assert "unrecognized arguments: --bogus-opt" in diagnostic["message"]
    run = project.run(["tests/test_a.py::test_a"], env=off)
    assert (run.status, run.outcomes) == (4, {})

    project.write(
        {
            "pytest.ini": "[pytest]\naddopts = -p helpers.opts --bogus-opt\n",
            "helpers/__init__.py": "",
            "helpers/opts.py": """
                def pytest_addoption(parser):
                    parser.addoption("--bogus-opt", action="store_true")
            """,
        }
    )
    _, run = same_as_pytest(project, env=off)
    assert run.outcomes == {"tests/test_a.py::test_a": "passed"}

    site = tmp_path / "site"
    info = site / "fakeplugin-1.0.dist-info"
    info.mkdir(parents=True)
    (info / "METADATA").write_text("Metadata-Version: 2.1\nName: fakeplugin\nVersion: 1.0\n")
    (info / "entry_points.txt").write_text("[pytest11]\nfake = fake_plugin\n")
    project.write({"pytest.ini": "[pytest]\naddopts = --bogus-opt\n"})
    refined = project.refine(project.files(), env={"PYTHONPATH": str(site)})
    assert ids(refined) == ["tests/test_a.py::test_a"]
    codes = {d["code"]: d for d in refined["diagnostics"]}
    assert sorted(codes) == ["plugin-not-loaded", "unknown-option"]
    assert codes["unknown-option"]["severity"] == "warning"
    assert "(fake)" in codes["unknown-option"]["message"]


# ------------------------------------------------------------------ the model


def test_closure_is_pytests_fixturenames(project):
    """Autouse first (by node, outermost first, then name), then
    ``usefixtures``, then the arguments; stably sorted by scope."""
    project.write(
        {
            "tests/test_closure.py": """
                import pytest

                @pytest.fixture(autouse=True)
                def zeta():
                    pass

                @pytest.fixture(autouse=True)
                def alpha():
                    pass

                @pytest.fixture(autouse=True, scope="module")
                def mod_m():
                    pass

                @pytest.fixture(scope="session")
                def s1():
                    pass

                @pytest.fixture
                def f1():
                    pass

                @pytest.mark.usefixtures("f1")
                def test_order(s1):
                    pass

                @pytest.mark.parametrize("p", [1])
                def test_direct(p, request):
                    pass
            """
        }
    )
    model = project.model(["tests/test_closure.py"])
    cases = {case["id"]: case for case in model["cases"]}
    order = cases["tests/test_closure.py::test_order"]
    assert order["closure"] == ["s1", "mod_m", "alpha", "zeta", "f1"]
    assert order["argnames"] == ["s1"]
    direct = cases["tests/test_closure.py::test_direct[1]"]
    # A direct parameter is a value, not a resource: pytest's fixturenames
    # lists it, the closure the engine resolves doesn't.
    assert direct["fixturenames"] == ["mod_m", "alpha", "zeta", "p", "request"]
    assert direct["closure"] == ["mod_m", "alpha", "zeta", "request"]
    assert direct["call_params"] == {"p": "1"}
    assert direct["param_scopes"] == {"p": "function"}


def test_override_chains_and_nodes(project):
    project.write(OUTCOMES)
    model = project.model(["tests/test_scopes.py"])
    cases = {case["id"]: case for case in model["cases"]}
    chain = cases["tests/test_scopes.py::TestOverride::test_chain"]
    # Closest definition first: class, module, the sub-conftest, the root's.
    assert chain["visible"]["base"] == [
        "tests/test_scopes.py::TestOverride",
        "tests/test_scopes.py",
        "tests/",
        "",
    ]
    assert chain["nodes"] == [
        ["tests/test_scopes.py", "module"],
        ["tests/test_scopes.py::TestOverride", "class"],
    ]
    assert chain["cls"] == "TestOverride"
    assert "_xunit_setup_module_fixture_test_scopes" in chain["closure"]
    assert "_xunit_setup_method_fixture_TestOverride" in chain["closure"]
    param = cases["tests/test_scopes.py::test_a[m1]"]
    assert param["resource_params"] == {"moduleparam": [0, "'m1'", "m1"]}
    assert param["param_scopes"] == {"moduleparam": "module"}
    # pytest's reorder_items: a module-scoped parameter set up once per value.
    assert [case["id"] for case in model["cases"]][2:6] == [
        "tests/test_scopes.py::test_a[m1]",
        "tests/test_scopes.py::test_b[m1]",
        "tests/test_scopes.py::test_a[m2]",
        "tests/test_scopes.py::test_b[m2]",
    ]


def test_unittest_cases_and_packages(project):
    project.write(
        {
            "tests/__init__.py": "",
            "tests/pkg/__init__.py": "",
            "tests/pkg/conftest.py": """
                import pytest

                @pytest.fixture(scope="package")
                def pkg():
                    return 1
            """,
            "tests/pkg/test_units.py": """
                import unittest

                class Units(unittest.TestCase):
                    def test_b(self):
                        pass

                    def test_a(self):
                        pass

                def test_pkg(pkg):
                    pass
            """,
        }
    )
    model = project.model(["tests/pkg/test_units.py"])
    cases = {case["id"]: case for case in model["cases"]}
    assert list(cases) == [
        "tests/pkg/test_units.py::Units::test_a",
        "tests/pkg/test_units.py::Units::test_b",
        "tests/pkg/test_units.py::test_pkg",
    ]
    unit = cases["tests/pkg/test_units.py::Units::test_a"]
    assert unit["kind"] == "unittest"
    assert unit["argnames"] == []
    assert unit["closure"] == ["_unittest_setUpClass_fixture_Units"]
    assert cases["tests/pkg/test_units.py::test_pkg"]["nodes"] == [
        ["tests", "package"],
        ["tests/pkg", "package"],
        ["tests/pkg/test_units.py", "module"],
    ]
    assert cases["tests/pkg/test_units.py::test_pkg"]["visible"]["pkg"] == ["tests/pkg/"]


def test_policy_hooks_and_rewriting(project):
    project.write(
        {
            "pytest.ini": """
                [pytest]
                xfail_strict = true
            """,
            "conftest.py": """
                def pytest_markeval_namespace(config):
                    return {"FLAG": True}

                def pytest_runtest_setup(item):
                    pass
            """,
            "tests/test_a.py": "def test_a():\n    pass\n",
            "tests/helpers.py": "",
        }
    )
    model = project.model(
        ["tests/test_a.py"], rewrite=["tests/test_a.py", "tests/helpers.py", "conftest.py"]
    )
    assert model["policy"]["xfail_strict"] is True
    assert {"FLAG", "config", "os", "platform", "sys"} <= set(model["policy"]["namespace"])
    assert model["hooks"] == {"setup": 1, "call": 0, "teardown": 1}
    assert model["rewrite"] == {
        "tests/test_a.py": True,
        "tests/helpers.py": False,
        "conftest.py": True,
    }


# ------------------------------------------------------------------ conftest hooks


def test_conftest_hooks_options_and_markers(project):
    project.write(
        {
            "pytest.ini": """
                [pytest]
                addopts = --strict-markers
            """,
            "conftest.py": """
                import pytest

                def pytest_addoption(parser):
                    parser.addoption("--speed", default="slow")

                def pytest_configure(config):
                    config.addinivalue_line("markers", "fast: quick ones")

                def pytest_collection_modifyitems(config, items):
                    if config.getoption("speed") == "fast":
                        items[:] = [i for i in items if i.get_closest_marker("fast")]
                    items.reverse()

                def pytest_ignore_collect(collection_path, config):
                    if collection_path.name == "test_ignored.py":
                        return True
            """,
            "tests/test_hooks.py": """
                import pytest

                @pytest.mark.fast
                def test_quick():
                    pass

                def test_slow():
                    pass
            """,
            "tests/test_ignored.py": "def test_never():\n    pass\n",
        }
    )
    refined = project.refine(["tests/test_hooks.py"])
    assert ids(refined) == project.pytest_ids()
    assert ids(refined) == ["tests/test_hooks.py::test_slow", "tests/test_hooks.py::test_quick"]
    expected = project.pytest()
    run = project.run(ids(refined))
    assert run.outcomes == expected["outcomes"]
    # The batch's order: the hooks ran once, over the whole session, when
    # the worker listed it (as in a pytest-xdist worker), so this is the
    # order plain pytest runs them in.
    assert run.started == ["tests/test_hooks.py::test_slow", "tests/test_hooks.py::test_quick"]
    # The arguments after ``--`` reach the conftest's option: pytest's hook
    # drops the slow test; refinement still lists it (a superset).
    fast = project.context(framework_args=["--speed", "fast"])
    refined = project.refine(["tests/test_hooks.py"], fast)
    assert ids(refined) == ["tests/test_hooks.py::test_quick", "tests/test_hooks.py::test_slow"]
    assert project.pytest_ids("--speed", "fast") == ["tests/test_hooks.py::test_quick"]
    run = project.run(ids(refined), fast)
    assert run.outcomes == {"tests/test_hooks.py::test_quick": "passed"}
    ignored = project.refine(["tests/test_ignored.py"])
    assert ignored["cases"] == []
    assert [d["code"] for d in ignored["diagnostics"]] == ["ignored"]


def test_unregistered_mark_with_strict_markers(project):
    project.write(
        {
            "pytest.ini": "[pytest]\naddopts = --strict-markers\n",
            "tests/test_typo.py": """
                import pytest

                @pytest.mark.slowish
                def test_typo():
                    pass
            """,
            "tests/test_fine.py": "def test_fine():\n    pass\n",
        }
    )
    refined = project.refine(["tests/test_typo.py", "tests/test_fine.py"])
    assert ids(refined) == ["tests/test_fine.py::test_fine"]
    (diagnostic,) = refined["diagnostics"]
    assert diagnostic["code"] == "collection-error"
    assert "'slowish' not found in `markers` configuration option" in diagnostic["message"]


def test_unsupported_hook_is_a_usage_error(project):
    project.write(
        {
            "tests/conftest.py": """
                def pytest_runtest_makereport(item, call):
                    pass
            """,
            "tests/test_a.py": "def test_a():\n    pass\n",
            "other/test_b.py": "def test_b():\n    pass\n",
        }
    )
    refined = project.refine(["tests/test_a.py", "other/test_b.py"])
    # Never a different result: the tests below that conftest aren't
    # collected, the others are.
    assert ids(refined) == ["other/test_b.py::test_b"]
    (diagnostic,) = refined["diagnostics"]
    assert diagnostic["code"] == "usage-error"
    assert "pytest_runtest_makereport" in diagnostic["message"]
    run = project.run(["tests/test_a.py::test_a", "other/test_b.py::test_b"])
    assert run.outcomes == {"other/test_b.py::test_b": "passed"}
    assert run.status == 4


def test_pytest_plugins_load_project_modules_only(project):
    project.write(
        {
            "conftest.py": 'pytest_plugins = ["helpers.fixtures"]\n',
            "helpers/__init__.py": "",
            "helpers/fixtures.py": """
                import pytest

                @pytest.fixture
                def answer():
                    return 42
            """,
            "tests/test_plugin.py": "def test_answer(answer):\n    assert answer == 42\n",
        }
    )
    same_as_pytest(project)
    project.write({"conftest.py": 'pytest_plugins = ["json"]\n'})
    refined = project.refine(["tests/test_plugin.py"])
    assert refined["cases"] == []
    assert [d["code"] for d in refined["diagnostics"]] == ["usage-error"]
    assert "third-party plugin" in refined["diagnostics"][0]["message"]


# ------------------------------------------------------------------ collection errors


def test_collection_errors_and_skips(project):
    project.write(
        {
            "tests/test_import.py": "import aye_no_such_module\n",
            "tests/test_syntax.py": "def test_x(:\n",
            "tests/test_skipped.py": """
                import pytest

                pytest.skip("not here", allow_module_level=True)

                def test_never():
                    pass
            """,
            "tests/test_bad_skip.py": """
                import pytest

                pytest.skip("oops")
            """,
            "tests/test_generator.py": """
                def test_gen():
                    yield 1

                def test_lost():
                    pass
            """,
            "tests/test_ok.py": "def test_ok():\n    pass\n",
            "tests/optional/conftest.py": 'import pytest\npytest.importorskip("aye_missing_dep")\n',
            "tests/optional/test_opt.py": "def test_opt():\n    pass\n",
        }
    )
    refined = project.refine(project.files())
    assert ids(refined) == ["tests/test_ok.py::test_ok"]
    found = {(d["path"], d["code"]): d["message"] for d in refined["diagnostics"]}
    assert set(found) == {
        ("tests/optional", "skipped"),
        ("tests/test_bad_skip.py", "collection-error"),
        ("tests/test_generator.py", "collection-error"),
        ("tests/test_import.py", "collection-error"),
        ("tests/test_skipped.py", "skipped"),
        ("tests/test_syntax.py", "collection-error"),
    }
    assert found["tests/test_import.py", "collection-error"].startswith(
        "ImportError while importing test module"
    )
    assert "allow_module_level=True" in found["tests/test_bad_skip.py", "collection-error"]
    assert (
        "'yield' keyword is allowed in fixtures"
        in found["tests/test_generator.py", "collection-error"]
    )
    assert found["tests/test_skipped.py", "skipped"] == "not here"
    run = project.run(
        [
            "tests/test_ok.py::test_ok",
            "tests/test_skipped.py::test_never",
            "tests/optional/test_opt.py::test_opt",
            "tests/test_generator.py::test_lost",
            "tests/test_ok.py::test_gone",
        ]
    )
    assert run.outcomes == {
        "tests/test_ok.py::test_ok": "passed",
        "tests/test_skipped.py::test_never": "skipped",
        "tests/optional/test_opt.py::test_opt": "skipped",
    }
    assert ("error", "not-found", "tests/test_ok.py") in run.diagnostics
    assert ("error", "collection-error", "tests/test_generator.py") in run.diagnostics
    # A file that failed to collect makes it pytest's status 1.
    assert run.status == 1


# ------------------------------------------------------------------ batches


def test_batches_keep_their_ids_and_order(project):
    project.write(
        {
            "tests/test_sel.py": """
                import pytest

                @pytest.mark.parametrize("x", [1, 2, 3])
                def test_p(x):
                    pass

                class TestK:
                    def test_one(self):
                        pass

                    def test_two(self):
                        pass

                def test_last():
                    pass
            """
        }
    )
    run = project.run(
        ["tests/test_sel.py::test_last", "tests/test_sel.py::TestK", "tests/test_sel.py::test_p"]
    )
    assert run.started == [
        "tests/test_sel.py::test_last",
        "tests/test_sel.py::TestK::test_one",
        "tests/test_sel.py::TestK::test_two",
        "tests/test_sel.py::test_p[1]",
        "tests/test_sel.py::test_p[2]",
        "tests/test_sel.py::test_p[3]",
    ]
    assert run.status == 0
    nothing = project.run(["tests/test_sel.py::test_gone"])
    assert nothing.started == []
    assert nothing.status == 5


def test_options_and_native_settings(project):
    """``-p no:tmpdir`` from ``addopts`` takes ``tmp_path`` away, as in
    pytest; the recognizer's ``native`` settings decide what a test file is."""
    project.write(
        {
            "pytest.ini": "[pytest]\naddopts = -p no:tmpdir\n",
            "tests/test_tmp.py": "def test_tmp(tmp_path):\n    pass\n",
            "tests/check_named.py": "def check_named():\n    pass\n",
        }
    )
    same_as_pytest(project, files=["tests/test_tmp.py"])
    native = project.context(
        native={
            "rootdir": str(project.root),
            "python_files": ["check_*.py"],
            "python_functions": ["check_"],
            "ini": {},
        }
    )
    refined = project.refine(["tests/check_named.py"], native)
    assert ids(refined) == ["tests/check_named.py::check_named"]


def test_importlib_mode_and_fixtures_on_static_methods(project):
    """``--import-mode=importlib`` imports a package's test modules under
    their package names (relative imports work, ``sys.path`` stays), and a
    fixture can be a ``staticmethod``."""
    project.write(
        {
            "pytest.ini": "[pytest]\naddopts = --import-mode=importlib\n",
            "tests/__init__.py": "",
            "tests/helpers.py": "VALUE = 3\n",
            "tests/conftest.py": """
                import pytest

                @pytest.fixture
                def base():
                    return 1
            """,
            "tests/test_relative.py": """
                import sys

                import pytest

                from .helpers import VALUE

                class TestStatic:
                    @pytest.fixture(name="kind", params=(int, str))
                    @staticmethod
                    def _kind(request):
                        return request.param

                    def test_kind(self, kind, base):
                        assert kind(VALUE) in (3, "3")
                        assert __name__ == "tests.test_relative"
            """,
            "other/test_plain.py": """
                def test_plain():
                    assert __name__ == "other.test_plain"
            """,
        }
    )
    refined, _ = same_as_pytest(project)
    assert ids(refined) == [
        "other/test_plain.py::test_plain",
        "tests/test_relative.py::TestStatic::test_kind[int]",
        "tests/test_relative.py::TestStatic::test_kind[str]",
    ]


def test_a_warning_filter_that_cant_apply_is_a_usage_error(project):
    project.write(
        {
            "pytest.ini": """
                [pytest]
                filterwarnings =
                    error::aye_no_such_module.SomeWarning
            """,
            "tests/test_a.py": "def test_a():\n    pass\n",
        }
    )
    refined = project.refine(["tests/test_a.py"])
    assert refined["cases"] == []
    (diagnostic,) = refined["diagnostics"]
    assert diagnostic["code"] == "usage-error"
    assert "aye_no_such_module.SomeWarning" in diagnostic["message"]


HOOKED = {
    "pytest.ini": """
        [pytest]
        usefixtures = session_marker
        markers =
            slow: slow tests
        xfail_strict = true
    """,
    "conftest.py": """
        import pytest

        ORDER = []

        def pytest_addoption(parser):
            parser.addoption("--runslow", action="store_true", default=False)

        def pytest_collection_modifyitems(config, items):
            if config.getoption("--runslow"):
                return
            skip_slow = pytest.mark.skip(reason="need --runslow")
            for item in items:
                if "slow" in item.keywords:
                    item.add_marker(skip_slow)

        def pytest_runtest_setup(item):
            marker = item.get_closest_marker("needs_env")
            if marker is not None:
                pytest.skip(f"needs {marker.args[0]}")

        @pytest.fixture(scope="session")
        def session_marker():
            ORDER.append("session")
            return "s"

        @pytest.fixture(scope="session", params=["a", "b"])
        def sparam(request):
            ORDER.append(f"sparam {request.param}")
            yield request.param
            ORDER.append(f"sparam end {request.param}")

        @pytest.fixture
        def order():
            return ORDER
    """,
    "tests/test_one.py": """
        import pytest

        pytestmark = [pytest.mark.filterwarnings("ignore::DeprecationWarning")]

        @pytest.mark.slow
        def test_slow():
            pass

        @pytest.mark.needs_env("db")
        def test_needs():
            pass

        def test_uses_sparam(sparam, order):
            assert sparam in ("a", "b")

        @pytest.fixture(scope="class")
        def per_class(order):
            order.append("class fixture")
            return object()

        def test_class_scope_without_class_1(per_class):
            pass

        def test_class_scope_without_class_2(per_class):
            pass

        @pytest.fixture(params=[1, 2], ids=lambda p: f"p{p}")
        def idfn_param(request):
            return request.param * 10

        def test_ids_callable(idfn_param):
            assert idfn_param in (10, 20)

        @pytest.fixture
        def doubled(request):
            return request.param * 2

        @pytest.mark.parametrize("doubled, plain", [(1, 2), (3, 6)], indirect=["doubled"])
        def test_indirect_list(doubled, plain):
            assert doubled == plain

        def test_getfixturevalue(request):
            assert request.getfixturevalue("session_marker") == "s"

        @pytest.mark.xfail(reason="strict from ini")
        def test_xpass_strict_from_ini():
            pass

        def test_importorskip_inside():
            pytest.importorskip("aye_no_such_module_anywhere")

        def test_marker_from_fixture(request):
            assert request.node.get_closest_marker("filterwarnings") is not None

        class TestXunit:
            log = []

            @classmethod
            def setup_class(cls):
                cls.log.append("setup_class")

            @classmethod
            def teardown_class(cls):
                cls.log.append("teardown_class")

            def setup_method(self, method):
                self.log.append(f"setup {method.__name__}")

            def teardown_method(self, method):
                self.log.append(f"teardown {method.__name__}")

            @pytest.fixture(autouse=True)
            def auto(self):
                self.log.append("auto")

            def test_first(self):
                assert self.log == ["setup_class", "setup test_first", "auto"]

            def test_second(self):
                assert self.log[-2:] == ["setup test_second", "auto"]

            class TestInner:
                def test_inner(self):
                    assert TestXunit.log[-1] == "teardown_class" or "auto" in TestXunit.log
    """,
    "tests/sub/test_two.py": """
        import pytest

        def test_also_sparam(sparam):
            assert sparam

        def pytest_generate_tests(metafunc):
            if "dyn" in metafunc.fixturenames:
                metafunc.parametrize("dyn", [metafunc.config.getoption("runslow"), "x"])

        def test_dyn(dyn):
            pass

        skipme = pytest.mark.skipif("config.getoption('runslow') is False", reason="string condition with config")

        @skipme
        def test_string_condition_config():
            pass
    """,
}


def test_conftest_hooks_marks_and_scopes_run_as_under_pytest(project):
    """Marks added by hooks, ``pytest_runtest_setup`` skips, an option a
    ``pytest_generate_tests`` reads, session-scoped parameters regrouped
    across files, class-scoped fixtures without a class, indirect lists,
    xunit methods with an autouse method fixture, ini ``usefixtures`` and
    ``xfail_strict``, string conditions reading ``config``."""
    project.write(HOOKED)
    same_as_pytest(project)
    runslow = project.context(framework_args=["--runslow"])
    same_as_pytest(project, context=runslow, args=["--runslow"])


# ------------------------------------------------------------------ one session per worker


def logged(project, tmp_path):
    """A log file the project's code appends to (``LOG(line)``), and the
    environment that names it."""
    path = tmp_path / "log.txt"
    path.write_text("", encoding="utf-8")
    return path, {"AYE_TEST_LOG": str(path)}


LOGGING = """
    import os

    def LOG(line):
        with open(os.environ["AYE_TEST_LOG"], "a", encoding="utf-8") as file:
            file.write(line + "\\n")
"""

SESSION = {
    "conftest.py": LOGGING
    + """
    import pytest

    def pytest_collection_modifyitems(items):
        LOG(f"modifyitems {len(items)}")

    def pytest_sessionfinish(session, exitstatus):
        LOG(f"sessionfinish {exitstatus}")

    @pytest.fixture(scope="session")
    def resource():
        LOG("session up")
        yield
        LOG("session down")

    @pytest.fixture(scope="module")
    def per_module():
        LOG("module up")
        yield
        LOG("module down")

    @pytest.fixture
    def marked(request):
        request.node.add_marker(pytest.mark.xfail(reason="a fixture said so"))
    """,
    "tests/test_a.py": LOGGING
    + """
    LOG("import test_a")

    def test_one(resource):
        pass

    def test_two(resource, per_module):
        pass
    """,
    "tests/test_b.py": LOGGING
    + """
    LOG("import test_b")

    def test_three(resource):
        pass

    def test_marked(marked):
        assert False
    """,
}


LOGGING = {
    "conftest.py": """
        import logging

        def pytest_sessionstart(session):
            logging.basicConfig(level="DEBUG", format="sessionstart %(message)s")
    """,
    "test_logging_setup.py": """
        import logging

        logging.basicConfig(level="NOTSET", format="module %(message)s")
        LEVEL = logging.getLogger().level

        def plain(logger):
            return [h for h in logger.handlers if type(h) is logging.StreamHandler]

        def test_basic_config_did_nothing():
            assert not plain(logging.getLogger())
            assert logging.getLogger().level == LOG_LEVEL

        def test_while_collecting_the_root_was_at_the_log_level():
            assert LEVEL == LOG_LEVEL

        def test_the_plugins_handlers_and_the_captures_are_on_the_root():
            assert len(logging.getLogger().handlers) == 4

        LOG_LEVEL = logging.WARNING
    """,
}


def test_the_logging_plugins_handlers_are_on_the_root_logger(project):
    """pytest's logging plugin keeps its handlers on the root logger around
    ``pytest_sessionstart``, collection and the test loop: a
    ``logging.basicConfig()`` there does nothing; with a ``log_level``, the
    root's level is lowered to it meanwhile."""
    project.write(LOGGING)
    _, run = same_as_pytest(project)
    assert set(run.outcomes.values()) == {"passed"}, run.events
    level = LOGGING["test_logging_setup.py"].replace(
        "LOG_LEVEL = logging.WARNING", "LOG_LEVEL = logging.INFO"
    )
    project.write({"pytest.ini": "[pytest]\nlog_level = INFO\n", "test_logging_setup.py": level})
    _, run = same_as_pytest(project)
    assert set(run.outcomes.values()) == {"passed"}, run.events


def test_a_worker_lists_once_and_its_session_outlives_its_batches(project, tmp_path):
    """Refinement lists the files; the batches run the cases it kept
    (nothing is imported again, the hooks ran once); the session's
    resources live until the worker closes, the module's close with each
    batch; ``pytest_sessionfinish`` runs once, at the close."""
    project.write(SESSION)
    log, env = logged(project, tmp_path)
    files = ["tests/test_a.py", "tests/test_b.py"]
    refined, first, second, _ = project.session(
        ("refine", files),
        ("run", ["tests/test_a.py::test_one", "tests/test_a.py::test_two"]),
        ("run", ["tests/test_b.py::test_three", "tests/test_b.py::test_marked"]),
        ("close",),
        env=env,
    )
    assert ids(refined) == [
        "tests/test_a.py::test_one",
        "tests/test_a.py::test_two",
        "tests/test_b.py::test_three",
        "tests/test_b.py::test_marked",
    ]
    assert first.outcomes == {
        "tests/test_a.py::test_one": "passed",
        "tests/test_a.py::test_two": "passed",
    }
    assert second.outcomes == {
        "tests/test_b.py::test_three": "passed",
        "tests/test_b.py::test_marked": "expected_failure",
    }
    assert (first.status, second.status) == (0, 0)
    assert log.read_text(encoding="utf-8").splitlines() == [
        "import test_a",
        "import test_b",
        "modifyitems 4",
        "session up",
        "module up",
        "module down",
        "session down",
        "sessionfinish 0",
    ]


def test_a_digests_only_listing_answers_digests_and_keeps_the_cases(project, tmp_path):
    project.write(SESSION)
    log, env = logged(project, tmp_path)
    files = ["tests/test_a.py", "tests/test_b.py"]
    (full,) = project.session(("refine", files), env=env)
    digests, run, _ = project.session(
        ("digests", files),
        ("run", ["tests/test_b.py::test_three", "tests/test_a.py::test_one"]),
        ("close",),
        env=env,
    )
    assert digests["cases"] == []
    expected = {}
    for file in files:
        found = [case["id"] for case in full["cases"] if case["id"].startswith(f"{file}::")]
        expected[file] = hashlib.sha1("\n".join(found).encode("utf-8")).hexdigest()
    assert digests["digests"] == expected == full["digests"]
    # The batch's order; nothing imported again.
    assert run.started == ["tests/test_b.py::test_three", "tests/test_a.py::test_one"]
    assert log.read_text(encoding="utf-8").count("import test_a") == 2  # one per worker


def test_a_batch_lists_the_files_its_worker_has_not_listed(project, tmp_path):
    project.write(SESSION)
    log, env = logged(project, tmp_path)
    _, run = project.session(
        ("refine", ["tests/test_a.py"]),
        ("run", ["tests/test_a.py::test_one", "tests/test_b.py::test_three"]),
        env=env,
    )
    assert run.outcomes == {
        "tests/test_a.py::test_one": "passed",
        "tests/test_b.py::test_three": "passed",
    }
    lines = log.read_text(encoding="utf-8").splitlines()
    assert lines[:4] == ["import test_a", "modifyitems 2", "import test_b", "modifyitems 2"]


def test_a_batch_fails_only_for_its_own_files(project):
    project.write(
        {
            "tests/test_ok.py": "def test_ok():\n    pass\n",
            "tests/test_broken.py": "import not_a_module_anywhere\n\ndef test_never():\n    pass\n",
        }
    )
    refined, run = project.session(
        ("refine", ["tests/test_ok.py", "tests/test_broken.py"]),
        ("run", ["tests/test_ok.py::test_ok"]),
    )
    assert [d["code"] for d in refined["diagnostics"]] == ["collection-error"]
    assert run.outcomes == {"tests/test_ok.py::test_ok": "passed"}
    assert run.status == 0
    assert run.diagnostics == []


AFFINITY = {
    "conftest.py": """
        import pytest

        @pytest.fixture(scope="session")
        def everywhere():
            return 1

        @pytest.fixture(scope="module")
        def per_module():
            return 1

        @pytest.fixture(scope="class")
        def per_class():
            return 1

        @pytest.fixture
        def per_test(everywhere):
            return 1

        @pytest.fixture
        def overridable():
            return 1
    """,
    "tests/test_free.py": """
        import pytest

        def test_plain(per_test, everywhere):
            pass

        def test_module_wide(per_module):
            pass

        @pytest.fixture
        def overridable(per_module, overridable):
            return overridable

        def test_through_an_override(overridable):
            pass

        @pytest.fixture
        def indirect(request):
            return request.param

        @pytest.mark.parametrize("indirect", [1], indirect=True, scope="module")
        def test_module_scoped_param(indirect):
            pass

        @pytest.mark.parametrize("value", [1], scope="module")
        def test_direct_param(value):
            pass

        class TestPlain:
            def test_method(self):
                pass

            def test_class_wide(self, per_class):
                pass

        def test_class_fixture_without_a_class(per_class):
            pass
    """,
    "tests/test_xunit.py": """
        def setup_module(module):
            pass

        def test_in_a_set_up_module():
            pass

        class TestSetUp:
            def setup_class(cls):
                pass

            def test_method(self):
                pass
    """,
    "tests/test_cases.py": """
        import unittest

        class Plain(unittest.TestCase):
            def test_it(self):
                pass

        class WithSetUpClass(unittest.TestCase):
            @classmethod
            def setUpClass(cls):
                pass

            def test_it(self):
                pass

        class WithClassCleanup(unittest.TestCase):
            def test_it(self):
                self.addClassCleanup(lambda: None)
    """,
    "tests/pkg/__init__.py": "",
    "tests/pkg/conftest.py": """
        import pytest

        @pytest.fixture(scope="package")
        def per_package():
            return 1
    """,
    "tests/pkg/test_package.py": """
        def test_package_wide(per_package):
            pass
    """,
}


def test_affinity_names_only_the_costly_lifetimes_a_case_shares(project):
    project.write(AFFINITY)
    refined = project.refine(project.files())
    affinity = {case["id"]: case["affinity"] for case in refined["cases"]}
    free, xunit, cases = "tests/test_free.py", "tests/test_xunit.py", "tests/test_cases.py"
    assert affinity == {
        f"{free}::test_plain": [],
        f"{free}::test_module_wide": [f"module:{free}"],
        f"{free}::test_through_an_override": [f"module:{free}"],
        f"{free}::test_module_scoped_param[1]": [f"module:{free}"],
        f"{free}::test_direct_param[1]": [],
        f"{free}::TestPlain::test_method": [],
        f"{free}::TestPlain::test_class_wide": [
            f"module:{free}",
            f"class:{free}::TestPlain",
        ],
        f"{free}::test_class_fixture_without_a_class": [],
        f"{xunit}::test_in_a_set_up_module": [f"module:{xunit}"],
        f"{xunit}::TestSetUp::test_method": [f"module:{xunit}", f"class:{xunit}::TestSetUp"],
        f"{cases}::Plain::test_it": [],
        f"{cases}::WithSetUpClass::test_it": [
            f"module:{cases}",
            f"class:{cases}::WithSetUpClass",
        ],
        f"{cases}::WithClassCleanup::test_it": [
            f"module:{cases}",
            f"class:{cases}::WithClassCleanup",
        ],
        "tests/pkg/test_package.py::test_package_wide": ["module:tests/pkg/test_package.py"],
    }
    # And every one of them runs as under pytest.
    same_as_pytest(project)


def test_affinity_keeps_the_import_key_of_files_the_import_mismatch_rule_groups(project):
    project.write(
        {
            "tests/a/test_util.py": "def test_a():\n    pass\n",
            "tests/b/test_other.py": "def test_b():\n    pass\n",
        }
    )
    context = project.context(colliding_modules=["test_util"])
    refined = project.refine(project.files(), context)
    affinity = {case["id"]: case["affinity"] for case in refined["cases"]}
    assert affinity == {
        "tests/a/test_util.py::test_a": ["import:test_util"],
        "tests/b/test_other.py::test_b": [],
    }


def test_closing_the_session_writes_what_failed_to_stderr(project):
    """With no batch to report into, a session's teardown error and a
    ``pytest_sessionfinish`` that raised go to the worker's stderr."""
    project.write(
        {
            "conftest.py": """
                import pytest

                def pytest_sessionfinish(session):
                    raise RuntimeError("sessionfinish broke")

                @pytest.fixture(scope="session")
                def resource():
                    yield
                    raise OSError("session teardown broke")
            """,
            "tests/test_a.py": "def test_a(resource):\n    pass\n",
        }
    )
    _, run, _ = project.session(
        ("refine", ["tests/test_a.py"]), ("run", ["tests/test_a.py::test_a"]), ("close",)
    )
    assert run.outcomes == {"tests/test_a.py::test_a": "passed"}
    assert "tearing down the session failed: OSError: session teardown broke" in project.stderr
    assert "sessionfinish broke" in project.stderr


SESSION_END = {
    "conftest.py": """
        import pytest

        @pytest.fixture(scope="session")
        def resource():
            yield
            raise OSError("session teardown broke")
    """,
    "tests/test_a.py": "def test_a(resource):\n    pass\n",
    "tests/test_b.py": "def test_b(resource):\n    pass\n",
}


def test_the_last_batch_ends_the_session_as_pytest_ends_it(project):
    """The batch the runner says is the worker's last ends its session, as
    a pytest-xdist worker ends it with its last test: the session's
    teardown error is that test's, as plain pytest reports it, and nothing
    is left for the worker's close."""
    project.write(SESSION_END)
    expected = project.pytest()
    _, first, last, _ = project.session(
        ("refine", project.files()),
        ("run", ["tests/test_a.py::test_a"]),
        ("last", ["tests/test_b.py::test_b"]),
        ("close",),
    )
    assert {**first.outcomes, **last.outcomes} == expected["outcomes"]
    assert expected["outcomes"]["tests/test_b.py::test_b"] == "error"
    assert (first.status, last.status) == (0, expected["exit_code"]) == (0, 1)
    assert "tearing down the session failed" not in project.stderr


def test_a_batch_without_tests_ends_the_session_and_reports_it(project):
    """A batch with no ids that ends the session (the worker's last batch
    didn't): what fails in the session's teardown has no test to blame, a
    ``lifetime_finished`` of its own, and fails the batch (status 1)."""
    project.write(SESSION_END)
    _, run, end, _ = project.session(
        ("refine", project.files()),
        ("run", ["tests/test_a.py::test_a", "tests/test_b.py::test_b"]),
        ("end",),
        ("close",),
    )
    assert set(run.outcomes.values()) == {"passed"}
    assert run.status == 0
    (lifetime,) = [e for e in end.of("lifetime_finished") if e.get("failure")]
    assert "session teardown broke" in lifetime["failure"]["message"]
    assert end.status == 1
    assert "tearing down the session failed" not in project.stderr


def test_a_session_pytest_sessionfinish_failed_fails_the_last_batch(project):
    """A ``session.exitstatus`` that ``pytest_sessionfinish`` sets to 1 when
    no test failed (a conftest's, pytest-cov's ``--cov-fail-under``) is the
    status of the batch that ends the session, with a ``session-failed``
    warning: pytest exits 1. A ``pytest_sessionfinish`` that raises is an
    internal error (status 3), as pytest's ``INTERNALERROR``."""
    project.write(
        {
            "conftest.py": """
                def pytest_sessionfinish(session, exitstatus):
                    session.exitstatus = 1
            """,
            "tests/test_a.py": "def test_a():\n    pass\n",
        }
    )
    expected = project.pytest()
    assert expected["exit_code"] == 1
    _, last = project.session(("refine", project.files()), ("last", ["tests/test_a.py::test_a"]))
    assert last.outcomes == expected["outcomes"] == {"tests/test_a.py::test_a": "passed"}
    assert last.status == 1
    assert last.diagnostics == [("warning", "session-failed", "")]
    project.write(
        {
            "conftest.py": """
                def pytest_sessionfinish(session, exitstatus):
                    raise RuntimeError("sessionfinish broke")
            """,
        }
    )
    _, last = project.session(("refine", project.files()), ("last", ["tests/test_a.py::test_a"]))
    assert last.status == 3
    assert last.diagnostics == [("error", "internal-error", ".")]


def test_an_items_fixture_values_go_away_after_its_teardown(project):
    """As pytest does once every teardown hook ran (``item.funcargs =
    None``): the teardown hooks see the values, a later test sees none, and
    nothing of the test that ran is held by its item."""
    project.write(
        {
            "conftest.py": """
                SEEN = []

                def pytest_runtest_teardown(item):
                    SEEN.append(sorted(item.funcargs))
            """,
            "test_a.py": """
                import conftest

                ITEMS = []

                def test_one(request, tmp_path):
                    ITEMS.append(request.node)
                    assert request.node.funcargs["tmp_path"] == tmp_path

                def test_two():
                    assert ITEMS[0].funcargs is None
                    assert conftest.SEEN == [["request", "tmp_path"]]
            """,
        }
    )
    same_as_pytest(project)


LATE_IMPORT = {
    "conftest.py": """
    import atexit
    import os

    import pytest

    def LOG(line):
        with open(os.environ["AYE_TEST_LOG"], "a", encoding="utf-8") as file:
            file.write(line + "\\n")

    pytest.register_assert_rewrite("helper_late")

    def pytest_configure(config):
        def at_exit():
            import helper_late

            try:
                helper_late.check()
            except AssertionError as exc:
                LOG(f"late: {exc}")

        atexit.register(at_exit)
    """,
    "helper_late.py": "def check():\n    x = 1\n    assert x == 2\n",
    "test_a.py": "def test_a():\n    pass\n",
}


def test_the_session_takes_its_rewriting_hook_out_when_it_ends(project, tmp_path):
    """pytest's configuration removes its import hook among its cleanups: a
    module imported afterwards (at exit) is Python's own, even one
    ``register_assert_rewrite`` named."""
    project.write(LATE_IMPORT)
    log, env = logged(project, tmp_path)
    project.pytest(env=env)  # collects, then runs: two sessions
    assert log.read_text(encoding="utf-8").splitlines() == ["late: ", "late: "]
    log.write_text("", encoding="utf-8")
    project.session(("refine", ["test_a.py"]), ("run", ["test_a.py::test_a"]), ("close",), env=env)
    assert log.read_text(encoding="utf-8").splitlines() == ["late: "]


ITEM_ATTRIBUTES = {
    "conftest.py": """
        import gc

        import pytest

        SEEN = {"thresholds": {gc.get_threshold()}}

        def pytest_generate_tests(metafunc):
            SEEN["thresholds"].add(gc.get_threshold())

        def pytest_make_parametrize_id(config, val, argname):
            SEEN["thresholds"].add(gc.get_threshold())

        def pytest_collection_modifyitems(items):
            SEEN["thresholds"].add(gc.get_threshold())
            first = items[0]
            SEEN["funcargs"] = first.funcargs
            SEEN["user_properties"] = first.user_properties
            SEEN["stash"] = first._store is first.stash
            first.funcargs["value"] = "preset"
            key = SEEN["key"] = pytest.StashKey()
            for item in items:
                item.stash[key] = item.name
                if item.name.startswith("test_extra"):
                    item.extra_keyword_matches.add("special")
            items[0].parent.extra_keyword_matches.add("from_the_module")
    """,
    "test_a.py": """
        import gc

        import conftest
        import pytest

        conftest.SEEN["thresholds"].add(gc.get_threshold())

        @pytest.mark.parametrize("value", range(3))
        def test_first(request, value):
            node = request.node
            assert node.stash[conftest.SEEN["key"]] == node.name
            node.user_properties.append(("value", value))
            # A value already in ``funcargs`` is the one the test gets.
            assert value == ("preset" if node.name == "test_first[0]" else value)

        def test_extra_one(request):
            assert "special" in request.node.listextrakeywords()
            assert "from_the_module" in request.node.listextrakeywords()

        def test_seen():
            seen = conftest.SEEN
            assert seen["thresholds"] == {gc.get_threshold()}
            # The item's own: filled while it ran.
            assert sorted(seen["funcargs"]) == ["request", "value"]
            assert seen["user_properties"] == [("value", "preset")]
            assert seen["stash"]
    """,
}


def test_items_attributes_and_the_collector_are_as_under_pytest(project):
    """What a conftest reads or changes on items while listing (``funcargs``,
    ``user_properties``, ``stash`` and ``_store``, ``extra_keyword_matches``)
    behaves as pytest's, and no code of the suite ever sees the garbage
    collector's settings other than its own."""
    project.write(ITEM_ATTRIBUTES)
    refined, _ = same_as_pytest(project)
    keywords = {case["id"]: case.get("keywords", []) for case in refined["cases"]}
    assert "special" in keywords["test_a.py::test_extra_one"]
    assert "special" not in keywords["test_a.py::test_seen"]
    assert all("from_the_module" in names for names in keywords.values())


KEYWORDS = {
    "conftest.py": """
        import json
        import os

        def pytest_collection_modifyitems(items):
            for item in items:
                if "extra" in item.name:
                    item.extra_keyword_matches.add("added_to_the_item")
            items[0].parent.extra_keyword_matches.add("added_to_the_module")

        def pytest_collection_finish(session):
            out = os.environ.get("AYE_TEST_KEYWORDS")
            if out:  # plain pytest, the referee: what ``-k`` matches
                from _pytest.mark import KeywordMatcher

                found = {
                    item.nodeid: sorted(KeywordMatcher.from_item(item)._names)
                    for item in session.items
                }
                with open(out, "w", encoding="utf-8") as file:
                    json.dump(found, file)
    """,
    "tests/test_k.py": """
        import pytest

        pytestmark = pytest.mark.module_wide

        def test_plain():
            pass

        test_plain.custom_attribute = True

        @pytest.mark.parametrize(
            "n", [1, pytest.param(2, marks=pytest.mark.second), pytest.param(3, id="three")]
        )
        def test_params(n):
            pass

        @pytest.mark.on_class
        class TestGroup:
            @pytest.mark.on_method
            def test_extra(self):
                pass

            @pytest.mark.parametrize("x", ["a", "b"])
            def test_extra_params(self, x):
                pass
    """,
    "tests/test_other.py": """
        def test_extra_elsewhere():
            pass
    """,
}


def test_the_names_k_matches_are_pytests(project, tmp_path):
    """Each refined case's ``keywords`` are the names pytest's ``-k``
    matches for it (``KeywordMatcher.from_item``): its chain's names, the
    extra keywords of it and its parents, its function's attributes and its
    marks, its own and inherited."""
    project.write(KEYWORDS)
    out = tmp_path / "keywords.json"
    project.pytest("--co", env={"AYE_TEST_KEYWORDS": str(out)})
    expected = json.loads(out.read_text(encoding="utf-8"))
    refined = project.refine(project.files())
    found = {case["id"]: case.get("keywords", []) for case in refined["cases"]}
    assert found == expected


RELEASED = {
    "conftest.py": """
        import atexit
        import gc
        import os
        import sys

        import pytest

        KEPT = []

        def pytest_collection_modifyitems(items):
            KEPT.extend(items)  # a conftest that holds the items

        @pytest.fixture
        def big():
            return list(range(1000))

        def check():
            gc.collect()
            model = sys.modules["aye_model"]
            engine = sys.modules["aye_native_engine"]
            alive = [type(o) for o in gc.get_objects()]
            with open(os.environ["AYE_TEST_LOG"], "a", encoding="utf-8") as file:
                file.write(
                    f"cases {alive.count(model.Case)} engines {alive.count(engine.Engine)} "
                    f"funcargs {[item.funcargs for item in KEPT]}\\n"
                )

        atexit.register(check)
    """,
    "test_a.py": """
        def test_one(big):
            assert len(big) == 1000

        def test_two(big):
            pass
    """,
}


def test_an_ended_session_holds_nothing_of_what_ran(project, tmp_path):
    """Once the session ends, what aye made for it (the cases, the engine,
    the values the tests got) is freed, even while the suite's code still
    holds its items: nothing of it is left for the collector to scan when
    the worker exits."""
    project.write(RELEASED)
    log, env = logged(project, tmp_path)
    project.session(
        ("refine", ["test_a.py"]),
        ("run", ["test_a.py::test_one", "test_a.py::test_two"]),
        ("close",),
        env=env,
    )
    assert log.read_text(encoding="utf-8").splitlines() == [
        "cases 0 engines 0 funcargs [None, None]"
    ]


GC_OFF = {
    "conftest.py": """
        import gc

        RAN = []

        def pytest_configure(config):
            gc.disable()
            gc.callbacks.append(lambda phase, info: RAN.append(phase))
    """,
    "test_a.py": """
        import gc

        import conftest
        import pytest

        @pytest.mark.parametrize("n", range(300))
        def test_many(n):
            pass

        def test_no_collection_ran():
            assert not gc.isenabled()
            assert conftest.RAN == []
    """,
}


def test_a_collector_the_suite_turned_off_never_runs(project):
    """Listing collects garbage only as the collector would by itself: not
    at all once the suite's code turned it off."""
    project.write(GC_OFF)
    same_as_pytest(project)


SELECTED = {
    "pytest.ini": "[pytest]\ntestpaths = tests\n",
    "tests/sub/conftest.py": """
        import os

        def pytest_sessionstart(session):
            with open(os.environ["AYE_TEST_LOG"], "a", encoding="utf-8") as file:
                file.write("sub sessionstart\\n")
    """,
    "tests/sub/test_x.py": "def test_x():\n    pass\n",
    "tests/test_y.py": "def test_y():\n    pass\n",
}


def test_a_batch_that_starts_from_other_paths_gets_its_own_session(project, tmp_path):
    """The paths a session starts from are part of it (``selected_paths``
    is not a batch's alone): ``pytest tests/sub/test_x.py`` loads that
    directory's conftest at start-up, so its ``pytest_sessionstart`` runs,
    which ``pytest`` from its ``testpaths`` never calls."""
    project.write(SELECTED)
    log, env = logged(project, tmp_path)
    project.pytest(env=env)  # collects, then runs: two sessions
    assert log.read_text(encoding="utf-8") == ""
    project.pytest("tests/sub/test_x.py", env=env)
    assert log.read_text(encoding="utf-8").splitlines() == ["sub sessionstart"] * 2
    log.write_text("", encoding="utf-8")
    listed = project.context()
    selected = {**listed, "selected_paths": [str(project.root / "tests/sub/test_x.py")]}
    files = ["tests/sub/test_x.py", "tests/test_y.py"]
    batch = {
        "id": 1,
        "adapter": "pytest",
        "ids": ["tests/sub/test_x.py::test_x"],
        "policy": {},
        "context": selected,
    }
    refined, run, _ = project._call(
        {
            "mode": "session",
            "steps": [
                {"op": "refine", "files": files, "context": listed},
                {"op": "run", "batch": batch},
                {"op": "close"},
            ],
        },
        env,
    )
    assert [case["id"] for case in refined["cases"]] == [
        "tests/sub/test_x.py::test_x",
        "tests/test_y.py::test_y",
    ]
    assert Run(run).outcomes == {"tests/sub/test_x.py::test_x": "passed"}
    assert log.read_text(encoding="utf-8").splitlines() == ["sub sessionstart"]


# ------------------------------------------------------------------ what failures tell

EXPLAINED = {
    "conftest.py": """
        import pytest


        @pytest.fixture(scope="session")
        def price_list():
            return {"apple": 2, "pear": 1}


        @pytest.fixture
        def cart(price_list):
            return {}


        @pytest.fixture
        def unreachable_database():
            raise ConnectionError("database unreachable")


        @pytest.fixture
        def fragile():
            yield 1
            raise RuntimeError("teardown failed")
    """,
    "test_explained.py": """
        def test_dict(cart, price_list):
            cart["apple"] = 4
            cart["pear"] = price_list["pear"] * 2
            assert cart == {"apple": 4, "pear": 3}


        def test_setup(price_list, unreachable_database):
            pass


        def test_teardown(fragile):
            assert fragile == 1


        def test_unknown(crat):
            pass


        def test_passes(cart):
            assert cart == {}
    """,
}


def test_failures_say_what_was_expected_where_and_which_fixture_broke(project):
    project.write(EXPLAINED)
    _, run = same_as_pytest(project)
    events = run.finished
    dict_case = events["test_explained.py::test_dict"]
    (failure,) = dict_case["failures"]
    assertion = failure["assertion"]
    assert assertion["expected"] == "right"
    assert (assertion["left_source"], assertion["right_source"]) == (
        "cart",
        '{"apple": 4, "pear": 3}',
    )
    assert assertion["compare"] == {
        "kind": "dict",
        "summary": "dict differs in 1 of 2 keys",
        "same": 1,
        "differs": [{"at": "['pear']", "left": "2", "right": "3"}],
    }
    frame = failure["frames"][-1]
    assert frame["source"][slice(*frame["cols"])] == "cart"
    assert frame["before"] == ['cart["apple"] = 4', 'cart["pear"] = price_list["pear"] * 2']
    assert frame["locals"] == [
        ["cart", "{'apple': 4, 'pear': 2}"],
        ["price_list", "{'apple': 2, 'pear': 1}"],
    ]
    assert dict_case["resources"] == [
        {"name": "price_list", "scope": "session"},
        {"name": "cart", "scope": "function"},
    ]
    setup = events["test_explained.py::test_setup"]
    assert (setup["failures"][0]["phase"], setup["failures"][0]["resource"]) == (
        "setup",
        "unreachable_database",
    )
    assert setup["resources"] == [
        {"name": "price_list", "scope": "session"},
        {"name": "unreachable_database", "scope": "function"},
    ]
    teardown = events["test_explained.py::test_teardown"]
    assert teardown["outcome"] == "error"
    assert [(f["phase"], f["resource"]) for f in teardown["failures"]] == [("teardown", "fragile")]
    unknown = events["test_explained.py::test_unknown"]["failures"][0]
    assert unknown["message"] == "fixture 'crat' not found"
    assert unknown["hint"] == "did you mean 'cart'?"
    assert "resources" not in events["test_explained.py::test_passes"]
