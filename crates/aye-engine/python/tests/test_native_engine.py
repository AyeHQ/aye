"""aye's native engine on hand-built model objects: no front-end, no pytest
module under test (pytest only runs these tests)."""

import dataclasses
import logging
import os
import sys
import unittest
import warnings
from pathlib import Path
from typing import ClassVar

import aye_engine
import aye_native_engine as ne
import pytest
from aye_model import Case, Hooks, Node, Policy, ResourceDef, Scope, Tag

# ------------------------------------------------------------------ helpers


class Skipped(BaseException):
    """A framework's skip exception, as the policy names it."""

    def __init__(self, msg=""):
        super().__init__(msg)
        self.msg = msg


class XFailed(BaseException):
    def __init__(self, msg=""):
        super().__init__(msg)
        self.msg = msg


class Exit(Exception):
    def __init__(self, msg="", returncode=None):
        super().__init__(msg)
        self.msg = msg
        self.returncode = returncode


POLICY = Policy(skip_exceptions=(Skipped,), xfail_exceptions=(XFailed,), exit_exceptions=(Exit,))


class Config:
    def __init__(self, options=None, ini=None):
        self.options = options or {}
        self.ini = ini or {}

    def getoption(self, name, default=None):
        return self.options.get(name, default)

    def getini(self, name):
        if name not in self.ini:
            raise ValueError(name)
        return self.ini[name]


def fixture(name, func, scope="function", **kwargs):
    argnames = kwargs.pop("argnames", None)
    if argnames is None:
        code = func.__code__
        argnames = code.co_varnames[: code.co_argcount]
        if kwargs.get("is_method"):
            argnames = argnames[1:]  # self, as a front-end leaves it out
    return ResourceDef(name, func, scope=Scope.named(scope), argnames=tuple(argnames), **kwargs)


def module(path, *defs, parent=None, tags=()):
    node = Node(path, "module", sys.modules[__name__], parent=parent, tags=list(tags))
    for definition in defs:
        node.visible.setdefault(definition.name, []).append(definition)
    return node


def child(parent, id, kind, obj=None, *defs):
    node = Node(
        id, kind, obj, parent=parent, visible={k: list(v) for k, v in parent.visible.items()}
    )
    for definition in defs:
        node.visible.setdefault(definition.name, []).insert(0, definition)
    return node


def case(node, func, *, name=None, closure=None, argnames=None, **kwargs):
    if argnames is None:
        code = func.__code__
        argnames = tuple(n for n in code.co_varnames[: code.co_argcount] if n != "self")
    if closure is None:
        closure = argnames
    test_id = f"{node.id}::{name or func.__name__}"
    return Case(test_id, node, func, closure=tuple(closure), argnames=tuple(argnames), **kwargs)


def run(cases, *, policy=POLICY, config=None, builtins=None, hooks=None, root="/repo", **kwargs):
    sink = aye_engine.ListSink(batch=0)
    engine_kwargs = {
        k: kwargs.pop(k) for k in ("current_test_env", "return_warning", "disabled") if k in kwargs
    }
    engine = ne.Engine(
        aye_engine.Paths(root),
        aye_engine.default_services(),
        policy,
        hooks=hooks,
        builtins=builtins,
        config=config,
        **engine_kwargs,
    )
    status = engine.run(cases, sink, **kwargs)
    return sink, status


def finished(sink):
    return {e["id"]: e for e in sink.events if e["event"] == "test_finished"}


def outcomes(sink):
    return {e["id"]: e["outcome"] for e in sink.events if e["event"] == "test_finished"}


def only(sink):
    (event,) = [e for e in sink.events if e["event"] == "test_finished"]
    return event


# ------------------------------------------------------------------ order and scopes


def test_setup_and_teardown_follow_scopes_across_modules_and_classes():
    log = []

    def session_fx():
        log.append("setup session")
        yield
        log.append("teardown session")

    def module_fx():
        log.append("setup module")
        yield
        log.append("teardown module")

    def class_fx():
        log.append("setup class")
        yield
        log.append("teardown class")

    def function_fx():
        log.append("setup function")
        yield
        log.append("teardown function")

    defs = [
        fixture("s", session_fx, "session"),
        fixture("m", module_fx, "module"),
        fixture("c", class_fx, "class"),
        fixture("f", function_fx),
    ]
    a = module("tests/test_a.py", *defs)
    klass = child(a, "tests/test_a.py::TestK", "class", type("TestK", (), {}))
    b = module("tests/test_b.py", *defs)

    def body(s, m, c, f):
        log.append("call")

    def method(self, s, m, c, f):
        log.append("method")

    TestK = klass.obj
    TestK.test_m = method
    cases = [
        case(a, body, name="test_1"),
        case(klass, method, name="test_m", cls=TestK),
        case(b, body, name="test_2"),
    ]
    sink, status = run(cases)
    assert status == 0
    assert set(outcomes(sink).values()) == {"passed"}
    assert log == [
        # test_1: a class-scoped resource on a function is function-scoped
        "setup session", "setup module", "setup class", "setup function", "call",
        "teardown function", "teardown class",
        # TestK::test_m: the module's instance carries on
        "setup class", "setup function", "method", "teardown function", "teardown class",
        "teardown module",
        # test_2: a new module
        "setup module", "setup class", "setup function", "call",
        "teardown function", "teardown class", "teardown module", "teardown session",
    ]  # fmt: skip


def test_finalizers_run_last_in_first_out():
    log = []

    def fx(request):
        request.addfinalizer(lambda: log.append("first added"))
        request.addfinalizer(lambda: log.append("second added"))
        yield
        log.append("after yield")

    def dependent(fx):
        yield
        log.append("dependent")

    m = module("t.py", fixture("fx", fx), fixture("dependent", dependent))

    def test(request, dependent):
        request.addfinalizer(lambda: log.append("test's own"))

    sink, _ = run([case(m, test, closure=("request", "dependent", "fx"))])
    assert only(sink)["outcome"] == "passed"
    assert log == ["test's own", "dependent", "after yield", "second added", "first added"]


def test_override_chain_gives_each_definition_the_next_one():
    def root():
        return "root"

    def mod(base):
        return base + "+mod"

    def cls(base):
        return base + "+cls"

    m = module("t.py")
    m.visible["base"] = [fixture("base", mod), fixture("base", root)]
    klass = child(m, "t.py::TestX", "class", type("TestX", (), {}), fixture("base", cls))
    seen = []

    def test(self, base):
        seen.append(base)

    def plain(base):
        seen.append(base)

    sink, _ = run([case(klass, test, cls=klass.obj), case(m, plain)])
    assert set(outcomes(sink).values()) == {"passed"}
    assert seen == ["root+mod+cls", "root+mod"]


def test_a_definition_requesting_itself_without_another_is_a_recursive_lookup_error():
    def loop(loop):
        return loop

    m = module("t.py", fixture("loop", loop))
    sink, _ = run([case(m, lambda loop: None, name="test", argnames=("loop",))])
    event = only(sink)
    assert event["outcome"] == "error"
    assert event["failures"][0]["message"].startswith(
        "recursive dependency involving fixture 'loop'"
    )


def test_parametrized_module_resource_is_recreated_when_its_param_changes():
    log = []

    def db(request):
        log.append(f"setup {request.param}")
        yield request.param
        log.append(f"teardown {request.param}")

    definition = fixture("db", db, "module", params=["m1", "m2"])
    m = module("t.py", definition)

    def record(name):
        def test(db):
            log.append(f"{name} {db}")

        return test

    cases = []
    for index, param in enumerate(["m1", "m2"]):
        for name in ("test_a", "test_b"):
            cases.append(
                case(
                    m,
                    record(name),
                    name=f"{name}[{param}]",
                    argnames=("db",),
                    resource_params={"db": (index, param, param)},
                    param_scopes={"db": Scope.MODULE},
                )
            )
    sink, _ = run(cases)
    assert set(outcomes(sink).values()) == {"passed"}
    assert log == [
        "setup m1", "test_a m1", "test_b m1",
        # torn down in the first m2 test's setup, as pytest does
        "teardown m1", "setup m2", "test_a m2", "test_b m2", "teardown m2",
    ]  # fmt: skip


def test_param_change_tears_down_dependents_first():
    log = []

    def base(request):
        log.append(f"base {request.param}")
        yield
        log.append(f"base down {request.param}")

    def user(base):
        log.append("user")
        yield
        log.append("user down")

    m = module(
        "t.py", fixture("base", base, "module", params=[1, 2]), fixture("user", user, "module")
    )
    cases = [
        case(
            m,
            lambda user: None,
            name=f"test[{p}]",
            argnames=("user",),
            closure=("base", "user"),
            resource_params={"base": (i, p, str(p))},
        )
        for i, p in enumerate([1, 2])
    ]
    sink, _ = run(cases)
    assert set(outcomes(sink).values()) == {"passed"}
    assert log == [
        "base 1",
        "user",
        "user down",
        "base down 1",
        "base 2",
        "user",
        "user down",
        "base down 2",
    ]


def test_request_param_is_missing_without_a_param_and_present_with_one():
    seen = {}

    def plain(request):
        seen["plain"] = hasattr(request, "param")
        return 1

    def indirect(request):
        seen["indirect"] = (request.param, request.param_index)
        return request.param

    m = module("t.py", fixture("plain", plain), fixture("indirect", indirect))
    run(
        [
            case(
                m,
                lambda plain, indirect: None,
                name="test[x]",
                argnames=("plain", "indirect"),
                resource_params={"indirect": (3, "x", "x")},
            )
        ]
    )
    assert seen == {"plain": False, "indirect": ("x", 3)}


def test_a_parametrized_resource_without_a_param_for_the_case_errors():
    m = module("t.py", fixture("p", lambda request: request.param, params=[1, 2]))
    sink, _ = run([case(m, lambda p: None, name="test", argnames=("p",))])
    event = only(sink)
    assert event["outcome"] == "error"
    assert "has no parameter defined for test" in event["failures"][0]["message"]


def test_direct_params_reach_the_test_and_the_resources_using_them():
    def doubled(x):
        return x * 2

    m = module("t.py", fixture("doubled", doubled))
    seen = []

    def test(x, doubled):
        seen.append((x, doubled))

    sink, _ = run([case(m, test, name="test[3]", closure=("x", "doubled"), call_params={"x": 3})])
    assert only(sink)["outcome"] == "passed"
    assert seen == [(3, 6)]


def test_getfixturevalue_sets_up_a_resource_the_closure_lacks():
    log = []

    def lazy():
        log.append("lazy up")
        yield "value"
        log.append("lazy down")

    m = module("t.py", fixture("lazy", lazy))
    names = []

    def test(request):
        assert request.getfixturevalue("lazy") == "value"
        assert request.getfixturevalue("lazy") == "value"
        names.extend(request.fixturenames)

    sink, _ = run([case(m, test)])
    assert only(sink)["outcome"] == "passed"
    assert log == ["lazy up", "lazy down"]
    assert "lazy" in names


def test_an_unknown_resource_is_a_setup_error_naming_what_is_available():
    m = module("t.py", fixture("present", lambda: 1))
    sink, _ = run([case(m, lambda missing: None, name="test", argnames=("missing",))])
    failure = only(sink)["failures"][0]
    assert only(sink)["outcome"] == "error"
    assert failure["phase"] == "setup"
    assert failure["message"] == "fixture 'missing' not found"
    assert failure["exception"] == "FixtureLookupError"
    assert "available fixtures: present" in failure["detail"]


def test_scope_mismatch_is_a_setup_error():
    m = module(
        "t.py",
        fixture("narrow", lambda: 1),
        fixture("wide", lambda narrow: narrow, "module"),
    )
    sink, _ = run(
        [case(m, lambda wide: None, name="test", argnames=("wide",), closure=("wide", "narrow"))]
    )
    event = only(sink)
    assert event["outcome"] == "error"
    assert event["failures"][0]["message"].startswith(
        "Failed: ScopeMismatch: You tried to access the function scoped fixture narrow "
        "with a module scoped request object."
    )


def test_a_failed_module_resource_is_cached_and_every_test_errors():
    calls = []

    def broken():
        calls.append(1)
        raise RuntimeError("no database")

    m = module("t.py", fixture("db", broken, "module"))
    cases = [case(m, lambda db: None, name=f"test_{i}", argnames=("db",)) for i in range(3)]
    sink, status = run(cases)
    assert status == 1
    assert set(outcomes(sink).values()) == {"error"}
    assert calls == [1]
    for event in finished(sink).values():
        assert event["failures"][0]["message"] == "RuntimeError: no database"


def test_a_wider_scope_teardown_error_lands_on_the_last_test_inside_it():
    def mod():
        yield
        raise RuntimeError("module teardown")

    m = module("t.py", fixture("mod", mod, "module"))
    other = module("u.py")
    cases = [
        case(m, lambda mod: None, name="test_uses", argnames=("mod",)),
        case(m, lambda: None, name="test_last_in_module", argnames=()),
        case(other, lambda: None, name="test_elsewhere", argnames=()),
    ]
    sink, _ = run(cases)
    assert outcomes(sink) == {
        "t.py::test_uses": "passed",
        "t.py::test_last_in_module": "error",
        "u.py::test_elsewhere": "passed",
    }
    failure = finished(sink)["t.py::test_last_in_module"]["failures"][0]
    assert failure["phase"] == "teardown"


def test_package_scope_closes_with_the_package_that_defines_it():
    log = []

    def pkg():
        log.append("pkg up")
        yield
        log.append("pkg down")

    definition = fixture("pkg", pkg, "package", baseid="tests/sub/")
    package = Node("tests/sub", "package", None)
    one = module("tests/sub/test_a.py", definition, parent=package)
    two = module("tests/sub/test_b.py", definition, parent=package)
    out = module("tests/test_c.py")
    cases = [
        case(one, lambda pkg: log.append("a"), name="test_a", argnames=("pkg",)),
        case(two, lambda pkg: log.append("b"), name="test_b", argnames=("pkg",)),
        case(out, lambda: log.append("c"), name="test_c", argnames=()),
    ]
    sink, _ = run(cases)
    assert set(outcomes(sink).values()) == {"passed"}
    assert log == ["pkg up", "a", "b", "pkg down", "c"]


def test_class_tests_get_a_fresh_instance_and_method_resources_bind_to_it():
    instances = []

    class TestX:
        def thing(self):
            return self

        def test_a(self, thing):
            instances.append((self, thing))

        def test_b(self, thing):
            instances.append((self, thing))

    m = module("t.py")
    klass = child(m, "t.py::TestX", "class", TestX, fixture("thing", TestX.thing, is_method=True))
    sink, _ = run([case(klass, TestX.test_a, cls=TestX), case(klass, TestX.test_b, cls=TestX)])
    assert set(outcomes(sink).values()) == {"passed"}
    (a, thing_a), (b, thing_b) = instances
    assert a is thing_a and b is thing_b and a is not b


def test_staticmethod_tests_stay_unbound():
    seen = []

    class TestS:
        @staticmethod
        def test_static(value):
            seen.append(value)

    m = module("t.py", fixture("value", lambda: 5))
    klass = child(m, "t.py::TestS", "class", TestS)
    sink, _ = run([case(klass, TestS.test_static, cls=TestS)])
    assert only(sink)["outcome"] == "passed"
    assert seen == [5]


def test_request_exposes_the_test_and_its_scope_node():
    seen = {}

    def mod_scope(request):
        seen["module_node"] = (request.node.nodeid, request.scope)
        seen["module_function"] = hasattr(request, "function")
        seen["module"] = request.module
        return 1

    def fn(request):
        seen["node"] = (request.node.name, request.node.nodeid, request.fixturename)
        seen["function"] = request.function
        seen["marker"] = request.node.get_closest_marker("slow")
        return 2

    m = module("tests/t.py", fixture("mod_scope", mod_scope, "module"), fixture("fn", fn))

    def test_it(mod_scope, fn):
        pass

    sink, _ = run([case(m, test_it, tags=[Tag("slow", (1,), {"a": 2})])])
    assert only(sink)["outcome"] == "passed"
    assert seen["module_node"] == ("tests/t.py", "module")
    assert seen["module_function"] is False
    assert seen["module"] is sys.modules[__name__]
    assert seen["node"] == ("test_it", "tests/t.py::test_it", "fn")
    assert seen["function"] is test_it
    assert (seen["marker"].name, seen["marker"].args, seen["marker"].kwargs) == (
        "slow",
        (1,),
        {"a": 2},
    )


def test_config_comes_from_the_engine_else_the_case():
    config = Config()
    seen = []
    m = module("t.py", fixture("cfg", lambda request: seen.append(request.config)))
    run([case(m, lambda cfg: None, name="test", argnames=("cfg",))], config=config)
    run([case(m, lambda cfg: None, name="test", argnames=("cfg",), extra={"config": "theirs"})])
    assert seen == [config, "theirs"]


# ------------------------------------------------------------------ one session, many runs


def engine_for(*, policy=POLICY, config=None, builtins=None, hooks=None, root="/repo"):
    """An engine for several ``run``s (a worker's)."""
    return ne.Engine(
        aye_engine.Paths(root),
        aye_engine.default_services(),
        policy,
        hooks=hooks,
        builtins=builtins,
        config=config,
    )


def lifecycle(log, name):
    def fx():
        log.append(f"{name} up")
        yield name
        log.append(f"{name} down")

    return fx


def test_a_kept_session_outlives_its_runs_until_close():
    log = []
    defs = [
        fixture("s", lifecycle(log, "session"), "session"),
        fixture("p", lifecycle(log, "package"), "package", baseid="tests/sub/"),
        fixture("m", lifecycle(log, "module"), "module"),
    ]
    package = Node("tests/sub", "package", None)
    a = module("tests/sub/test_a.py", *defs, parent=package)
    b = module("tests/sub/test_b.py", *defs, parent=package)
    other = module("tests/test_c.py", *defs)

    def test(s, p, m):
        log.append("test")

    def plain(s, m):
        log.append("plain")

    engine = engine_for()
    first, second, third = (aye_engine.ListSink(batch=n) for n in range(3))
    assert engine.run([case(a, test, name="test_a")], first, keep_session=True) == 0
    # The module's lifetime closed with the batch; the session's and the
    # package's stay open.
    assert log == ["session up", "package up", "module up", "test", "module down"]
    assert engine.run([case(b, test, name="test_b")], second, keep_session=True) == 0
    assert log[5:] == ["module up", "test", "module down"]
    # A batch outside the package closes it first, with no case to blame.
    assert engine.run([case(other, plain, name="test_c")], third, keep_session=True) == 0
    assert log[8:] == ["package down", "module up", "plain", "module down"]
    assert engine.close() == []
    assert log[12:] == ["session down"]
    for sink in (first, second, third):
        assert set(outcomes(sink).values()) == {"passed"}
        assert not [e for e in sink.events if e["event"].startswith("lifetime")]


def test_closing_a_kept_session_reports_its_teardown_errors():
    def broken():
        yield
        raise OSError("session teardown failed")

    m = module("t.py", fixture("s", broken, "session"))
    engine = engine_for()
    sink = aye_engine.ListSink(batch=0)
    engine.run([case(m, lambda s: None, name="test", argnames=("s",))], sink, keep_session=True)
    assert only(sink)["outcome"] == "passed"
    closing = aye_engine.ListSink()
    failures = engine.close(closing)
    assert [f.exception for f in failures] == ["OSError"]
    finished_lifetime = [e for e in closing.events if e["event"] == "lifetime_finished"]
    assert finished_lifetime[0]["failure"]["message"] == "OSError: session teardown failed"
    # Closed once: a second close has nothing left.
    assert engine.close() == []


def test_a_package_left_open_that_fails_to_close_is_reported_by_the_next_run():
    def pkg():
        yield
        raise OSError("package teardown failed")

    definition = fixture("p", pkg, "package", baseid="tests/sub/")
    inside = module("tests/sub/test_a.py", definition, parent=Node("tests/sub", "package", None))
    outside = module("tests/test_b.py")
    engine = engine_for()
    engine.run(
        [case(inside, lambda p: None, name="test_a", argnames=("p",))],
        aye_engine.ListSink(batch=0),
        keep_session=True,
    )
    sink = aye_engine.ListSink(batch=1)
    engine.run([case(outside, lambda: None, name="test_b", argnames=())], sink, keep_session=True)
    assert only(sink)["outcome"] == "passed"
    lifetime = [e for e in sink.events if e["event"] == "lifetime_finished"]
    assert lifetime[0]["failure"]["exception"] == "OSError"


def test_a_stop_closes_a_kept_session_at_once():
    log = []
    m = module("t.py", fixture("s", lifecycle(log, "session"), "session"))

    def stops(s):
        raise Exit("enough", returncode=3)

    engine = engine_for()
    sink = aye_engine.ListSink(batch=0)
    status = engine.run([case(m, stops, name="test")], sink, keep_session=True)
    assert status == 3
    assert log == ["session up", "session down"]


def test_without_keep_session_a_run_closes_everything():
    log = []
    m = module("t.py", fixture("s", lifecycle(log, "session"), "session"))
    engine = engine_for()
    engine.run([case(m, lambda s: None, name="test", argnames=("s",))], aye_engine.ListSink())
    assert log == ["session up", "session down"]
    engine.run([case(m, lambda s: None, name="test", argnames=("s",))], aye_engine.ListSink())
    assert log[2:] == ["session up", "session down"]


def test_an_xfail_condition_is_evaluated_once_per_case():
    calls = []

    class Condition:
        def __bool__(self):
            calls.append(1)
            return False

    m = module("t.py")
    tags = [Tag("xfail", (Condition(),), {"reason": "never"})]
    sink, _ = run([case(m, lambda: None, name="test", argnames=(), tags=tags)])
    assert only(sink)["outcome"] == "passed"
    assert calls == [1]


def test_each_case_gets_only_its_own_output_from_the_reused_capture():
    held = []

    def first():
        print("first")
        held.append(sys.stdout)

    def second():
        print("second")

    def closes():
        print("closing")
        sys.stdout.close()

    def after_close():
        print("still captured")

    def late():
        held[0].write("late write\n")

    m = module("t.py")
    tests = (first, second, late, closes, after_close)
    sink, _ = run([case(m, t, name=t.__name__, argnames=()) for t in tests])
    out = {e["id"]: e.get("output", {}).get("stdout") for e in finished(sink).values()}
    assert out == {
        "t.py::first": "first\n",
        "t.py::second": "second\n",
        # A stream a test kept is the capture's still, as pytest's global
        # capture: what it writes later is the running test's.
        "t.py::late": "late write\n",
        # A test that closes its stream gets the next one a new one.
        "t.py::closes": "closing\n",
        "t.py::after_close": "still captured\n",
    }


# ------------------------------------------------------------------ conditions


def body_log():
    calls = []

    def test():
        calls.append("called")

    return calls, test


@pytest.mark.parametrize(
    ("tags", "outcome", "reason"),
    [
        ([Tag("skip", (), {"reason": "not today"})], "skipped", "not today"),
        ([Tag("skip", ("positional",))], "skipped", "positional"),
        ([Tag("skip")], "skipped", "unconditional skip"),
        ([Tag("skipif", (True,), {"reason": "yes"})], "skipped", "yes"),
        ([Tag("skipif", (False,), {"reason": "no"})], "passed", None),
        ([Tag("skipif", (False, True), {"reason": "second"})], "skipped", "second"),
        ([Tag("skipif", ("1 + 1 == 2",))], "skipped", "condition: 1 + 1 == 2"),
        ([Tag("skipif", ("sys.platform == 'nowhere'",))], "passed", None),
        ([Tag("skipif", ("MODULE_FLAG",))], "skipped", "condition: MODULE_FLAG"),
        ([Tag("skipif", (), {"condition": True, "reason": "kw"})], "skipped", "kw"),
        ([Tag("skipif", (), {"reason": "bare"})], "skipped", "bare"),
        ([Tag("skipif", (True,))], "error", None),
        ([Tag("skipif", ("1 +",))], "error", None),
        ([Tag("skipif", ("undefined_name",))], "error", None),
        (
            [Tag("skipif", (False,), {"reason": "x"}), Tag("skip", (), {"reason": "then skip"})],
            "skipped",
            "then skip",
        ),
    ],
)
def test_skip_conditions(tags, outcome, reason):
    calls, test = body_log()
    setups = []
    m = module("t.py", fixture("auto", lambda: setups.append(1), autouse=True))
    sink, _ = run([case(m, test, name="test", argnames=(), closure=("auto",), tags=tags)])
    event = only(sink)
    assert event["outcome"] == outcome
    assert event.get("reason") == reason
    if outcome != "passed":
        assert calls == [] and setups == []  # nothing runs, autouse included


MODULE_FLAG = True


@pytest.mark.parametrize(
    ("kwargs", "body", "outcome", "reason"),
    [
        ({"reason": "broken"}, "fail", "expected_failure", "broken"),
        ({"reason": "broken"}, "pass", "unexpected_pass", "broken"),
        ({"reason": "broken", "strict": True}, "pass", "failed", None),
        ({"reason": "wrong", "raises": KeyError}, "fail", "failed", None),
        ({"reason": "right", "raises": ValueError}, "fail", "expected_failure", "right"),
        (
            {"reason": "tuple", "raises": (KeyError, ValueError)},
            "fail",
            "expected_failure",
            "tuple",
        ),
        ({"reason": "later", "run": False}, "pass", "expected_failure", "[NOTRUN] later"),
        ({"condition": False, "reason": "off"}, "fail", "failed", None),
        ({"condition": "True"}, "fail", "expected_failure", "condition: True"),
        ({}, "skip", "skipped", "inside"),
        ({}, "xfail", "expected_failure", "imperative"),
    ],
)
def test_xfail_matrix(kwargs, body, outcome, reason):
    calls = []

    def test():
        calls.append(1)
        if body == "fail":
            raise ValueError("boom")
        if body == "skip":
            raise Skipped("inside")
        if body == "xfail":
            raise XFailed("imperative")

    tags = [Tag("xfail", (), kwargs)]
    sink, _ = run([case(module("t.py"), test, name="test", argnames=(), tags=tags)])
    event = only(sink)
    assert event["outcome"] == outcome
    assert event.get("reason") == reason
    if kwargs.get("run") is False:
        assert calls == []
    if kwargs.get("strict"):
        assert event["failures"][0]["message"] == "[XPASS(strict)] broken"


def test_xfail_strict_comes_from_the_policy():
    policy = Policy(xfail_strict=True)
    sink, _ = run(
        [case(module("t.py"), lambda: None, name="test", argnames=(), tags=[Tag("xfail")])],
        policy=policy,
    )
    assert only(sink)["outcome"] == "failed"


def test_xfail_turns_a_failing_resource_into_an_expected_failure():
    def broken():
        raise RuntimeError("setup")

    m = module("t.py", fixture("broken", broken))
    sink, _ = run(
        [
            case(
                m,
                lambda broken: None,
                name="test",
                argnames=("broken",),
                tags=[Tag("xfail", (), {"reason": "r"})],
            )
        ]
    )
    assert only(sink)["outcome"] == "expected_failure"


def test_an_xfail_tag_added_while_running_applies_to_the_call():
    def marker(request):
        request.applymarker(Tag("xfail", (), {"reason": "added"}))

    m = module("t.py", fixture("marker", marker))

    def test(marker):
        raise AssertionError("expected")

    sink, _ = run([case(m, test)])
    assert only(sink)["outcome"] == "expected_failure"
    assert only(sink)["reason"] == "added"


def test_a_mark_decorator_like_object_can_be_applied():
    class Mark:
        name, args, kwargs = "xfail", (), {"reason": "decorated"}

    class Decorator:
        mark = Mark()

    def test(request):
        request.applymarker(Decorator())
        raise ValueError

    sink, _ = run([case(module("t.py"), test)])
    assert only(sink)["outcome"] == "expected_failure"


# ------------------------------------------------------------------ folding


def make(raise_in):
    log = []

    def fx():
        if raise_in == "setup":
            raise RuntimeError("in setup")
        if raise_in == "setup-skip":
            raise Skipped("skipped in setup")
        yield
        if raise_in in ("teardown", "call+teardown"):
            raise RuntimeError("in teardown")
        if raise_in == "teardown-skip":
            raise Skipped("skipped in teardown")

    def test(fx):
        log.append("called")
        if raise_in in ("call", "call+teardown"):
            raise ValueError("in call")
        if raise_in == "assert":
            raise AssertionError("assert 1 == 2")
        if raise_in == "unittest-skip":
            raise unittest.SkipTest("unittest's")
        if raise_in == "aye-skip":
            raise aye_engine.Skip("aye's")

    m = module("t.py", fixture("fx", fx))
    return log, case(m, test)


@pytest.mark.parametrize(
    ("raise_in", "outcome", "phases", "called"),
    [
        (None, "passed", [], True),
        ("setup", "error", ["setup"], False),
        ("setup-skip", "skipped", [], False),
        ("call", "failed", ["test"], True),
        ("assert", "failed", ["test"], True),
        ("teardown", "error", ["teardown"], True),
        ("call+teardown", "error", ["test", "teardown"], True),
        ("teardown-skip", "skipped", [], True),
        ("unittest-skip", "skipped", [], True),
        ("aye-skip", "skipped", [], True),
    ],
)
def test_phases_fold_into_one_outcome(raise_in, outcome, phases, called):
    log, the_case = make(raise_in)
    sink, status = run([the_case])
    event = only(sink)
    assert event["outcome"] == outcome
    assert [f["phase"] for f in event.get("failures", [])] == phases
    assert (log == ["called"]) is called
    assert status == (1 if outcome in ("failed", "error") else 0)


def test_a_failure_carries_the_users_frames_and_message():
    def test():
        helper()

    def helper():
        raise ValueError("deep\nsecond line")

    sink, _ = run([case(module("t.py"), test)], root=os.path.dirname(__file__))
    failure = only(sink)["failures"][0]
    assert failure["message"] == "ValueError: deep"
    assert failure["exception"] == "ValueError"
    assert [f["function"] for f in failure["frames"]] == ["test", "helper"]
    assert failure["location"]["path"] == "test_native_engine.py"
    assert "aye_native_engine" not in failure["detail"]


def test_bad_outcomes_carry_reproduce():
    def reproduce(test_id):
        return {"command": ["aye", test_id]}

    m = module("t.py")
    sink, _ = run(
        [
            case(m, lambda: None, name="ok", argnames=()),
            case(m, lambda: 1 / 0, name="bad", argnames=()),
        ],
        reproduce=reproduce,
    )
    events = finished(sink)
    assert "reproduce" not in events["t.py::ok"]
    assert events["t.py::bad"]["reproduce"] == {"command": ["aye", "t.py::bad"]}


def test_async_tests_fail_as_unsupported():
    async def test():
        pass

    sink, _ = run([case(module("t.py"), test)])
    event = only(sink)
    assert event["outcome"] == "failed"
    assert (
        event["failures"][0]["message"] == "Failed: async def functions are not natively supported."
    )


def test_the_innermost_project_frame_shows_its_surroundings():
    def test():
        payload = {"items": [1, 2]}
        helper(payload)

    def helper(payload):
        count = len(payload["items"])
        raise ValueError(f"{count} items")

    sink, _ = run([case(module("t.py"), test)], root=os.path.dirname(__file__))
    outer, inner = only(sink)["failures"][0]["frames"]
    assert "locals" not in outer and "before" not in outer
    assert inner["locals"] == [["count", "2"], ["payload", "{'items': [1, 2]}"]]
    assert inner["before"] == ["def helper(payload):", '    count = len(payload["items"])']
    assert inner["indent"] == 4


# ------------------------------------------------------------------ what a failure tells


def test_a_bad_outcome_lists_its_resources_in_setup_order_with_their_scopes():
    m = module(
        "t.py",
        fixture("price_list", lambda: {"pear": 3}, "session"),
        fixture("shop", lambda price_list: price_list, "module"),
        fixture("cart", lambda shop, request: [], argnames=("shop", "request")),
    )

    def test_fails(cart, n):
        raise AssertionError("no")

    def test_passes(cart):
        pass

    cases = [
        case(m, test_fails, closure=("price_list", "shop", "cart", "n"), call_params={"n": 1}),
        case(m, test_passes, closure=("price_list", "shop", "cart")),
    ]
    events = finished(run(cases)[0])
    # ``request`` and direct parameters aren't resources.
    assert events["t.py::test_fails"]["resources"] == [
        {"name": "price_list", "scope": "session"},
        {"name": "shop", "scope": "module"},
        {"name": "cart", "scope": "function"},
    ]
    assert "resources" not in events["t.py::test_passes"]


def test_a_parametrized_resource_has_its_parameters_scope():
    def db(request):
        return request.param

    m = module("t.py", fixture("db", db, argnames=("request",)))
    cases = [
        case(
            m,
            lambda db: 1 / 0,
            name="test[a]",
            argnames=("db",),
            resource_params={"db": (0, "a", "a")},
            param_scopes={"db": Scope.MODULE},
        )
    ]
    assert only(run(cases)[0])["resources"] == [{"name": "db", "scope": "module"}]


def test_a_setup_failure_names_the_resource_that_raised_it():
    def database():
        raise ConnectionError("database unreachable")

    def repository(database):
        return database

    m = module(
        "t.py",
        fixture("database", database, "module"),
        fixture("repository", repository, "module"),
        fixture("clock", lambda: 0),
    )
    cases = [
        case(
            m,
            lambda clock, repository: None,
            name=f"test_{i}",
            argnames=("clock", "repository"),
            closure=("clock", "repository", "database"),
        )
        for i in range(2)
    ]
    events = finished(run(cases)[0])
    for event in events.values():
        # The resource that raised, not the one that needed it; again for
        # the next test, which gets the failure cached.
        failure = event["failures"][0]
        assert (failure["phase"], failure["resource"]) == ("setup", "database")
        # Those set up first, the one that failed, then the rest.
        assert event["resources"] == [
            {"name": "clock", "scope": "function"},
            {"name": "repository", "scope": "module"},
            {"name": "database", "scope": "module"},
        ]


def test_a_resource_that_raises_what_another_raised_is_blamed_for_its_own():
    def inner():
        raise KeyError("inner")

    def outer(request):
        try:
            request.getfixturevalue("inner")
        except KeyError:
            raise RuntimeError("outer") from None

    m = module("t.py", fixture("inner", inner), fixture("outer", outer, argnames=("request",)))
    sink, _ = run([case(m, lambda outer: None, name="test", argnames=("outer",))])
    assert only(sink)["failures"][0]["resource"] == "outer"


def test_a_teardown_failure_names_its_resource_and_the_test_passed():
    def fragile():
        yield "value"
        raise RuntimeError("teardown failed")

    def sturdy(fragile):
        yield fragile

    m = module("t.py", fixture("fragile", fragile), fixture("sturdy", sturdy))
    sink, _ = run([case(m, lambda sturdy: None, name="test", argnames=("sturdy",))])
    event = only(sink)
    assert event["outcome"] == "error"
    (failure,) = event["failures"]
    assert (failure["phase"], failure["resource"]) == ("teardown", "fragile")
    assert failure["message"] == "RuntimeError: teardown failed"


def test_a_body_failure_names_no_resource():
    m = module("t.py", fixture("fx", lambda: 1))
    sink, _ = run([case(m, lambda fx: 1 / 0, name="test", argnames=("fx",))])
    assert "resource" not in only(sink)["failures"][0]


def test_an_unknown_resource_gets_the_closest_names_as_a_hint():
    m = module(
        "t.py",
        fixture("cart", lambda: 1),
        fixture("card", lambda: 1),
        fixture("database", lambda: 1),
    )

    def hint(name):
        sink, _ = run([case(m, lambda: None, name="test", argnames=(name,))])
        failure = only(sink)["failures"][0]
        assert failure["message"] == f"fixture {name!r} not found"
        assert "available fixtures: card, cart, database" in failure["detail"]
        return failure.get("hint")

    assert hint("databse") == "did you mean 'database'?"
    assert hint("carts") == "did you mean 'cart' or 'card'?"
    assert hint("nothing_like_it") is None
    builtins = {"tmp_path": fixture("tmp_path", lambda: 1)}
    sink, _ = run([case(m, lambda: None, name="test", argnames=("tmp_pth",))], builtins=builtins)
    assert only(sink)["failures"][0]["hint"] == "did you mean 'tmp_path'?"


def test_a_recursive_lookup_has_no_hint():
    def loop(loop):
        return loop

    m = module("t.py", fixture("loop", loop))
    sink, _ = run([case(m, lambda loop: None, name="test", argnames=("loop",))])
    assert "hint" not in only(sink)["failures"][0]


# ------------------------------------------------------------------ assertions


def test_a_rewritten_comparison_carries_its_expected_side_and_how_the_values_differ():
    def test():
        exc = AssertionError("assert {'pear': 2} == {'pear': 3}")
        exc._aye_assert = {
            "op": "==",
            "left": {"apple": 4, "pear": 2},
            "right": {"apple": 4, "pear": 3},
            "expr": 'cart.summary() == {"apple": 4, "pear": 3}',
            "left_src": "cart.summary()",
            "right_src": '{"apple": 4, "pear": 3}',
            "left_cols": (11, 25),
            "right_cols": (29, 52),
            "expected": "right",
        }
        raise exc

    assertion = only(run([case(module("t.py"), test)])[0])["failures"][0]["assertion"]
    assert assertion["expected"] == "right"
    assert assertion["left_source"] == "cart.summary()"
    assert assertion["right_source"] == '{"apple": 4, "pear": 3}'
    assert assertion["compare"] == {
        "kind": "dict",
        "summary": "dict differs in 1 of 2 keys",
        "same": 1,
        "differs": [{"at": "['pear']", "left": "2", "right": "3"}],
    }


def test_only_equality_is_compared():
    def test():
        exc = AssertionError("assert 5 <= 3")
        exc._aye_assert = {"op": "<=", "left": 5, "right": 3, "expr": "n <= LIMIT"}
        raise exc

    assertion = only(run([case(module("t.py"), test)])[0])["failures"][0]["assertion"]
    assert assertion["op"] == "<="
    assert "compare" not in assertion and "expected" not in assertion


def test_a_rewritten_comparison_becomes_a_structured_assertion(monkeypatch):
    # pytest shows full diffs on CI (CI or BUILD_NUMBER set), and so does
    # the engine: this pins the terminal's shorter form.
    monkeypatch.delenv("CI", raising=False)
    monkeypatch.delenv("BUILD_NUMBER", raising=False)

    def test():
        exc = AssertionError("assert {'a': 1, 'b': 2} == {'a': 1, 'b': 3}")
        exc._aye_assert = {
            "op": "==",
            "left": {"a": 1, "b": 2},
            "right": {"a": 1, "b": 3},
            "expr": "x == y",
        }
        raise exc

    sink, _ = run([case(module("t.py"), test)])
    failure = only(sink)["failures"][0]
    assert failure["message"] == "assert {'a': 1, 'b': 2} == {'a': 1, 'b': 3}"
    assert failure["assertion"] == {
        "op": "==",
        "left": "{'a': 1, 'b': 2}",
        "right": "{'a': 1, 'b': 3}",
        "diff": [
            "Omitting 1 identical items, use -vv to show",
            "Differing items:",
            "{'b': 2} != {'b': 3}",
            "Use -v to get more diff",
        ],
        "compare": {
            "kind": "dict",
            "summary": "dict differs in 1 of 2 keys",
            "same": 1,
            "differs": [{"at": "['b']", "left": "2", "right": "3"}],
        },
    }


@pytest.fixture
def no_ci(monkeypatch):
    monkeypatch.delenv("CI", raising=False)
    monkeypatch.delenv("BUILD_NUMBER", raising=False)


@pytest.mark.usefixtures("no_ci")
@pytest.mark.parametrize(
    ("op", "left", "right", "lines"),
    [
        ("==", 1, 2, []),
        ("==", [1, 2, 3], [1, 2, 4], ["At index 2 diff: 3 != 4", "Use -v to get more diff"]),
        ("==", [1, 2], [1, 2, 3], ["Right contains one more item: 3", "Use -v to get more diff"]),
        (
            "==",
            (1, 2, 3, 4),
            (1,),
            ["Left contains 3 more items, first extra item: 2", "Use -v to get more diff"],
        ),
        (
            "==",
            {1, 2},
            {2, 3},
            [
                "Extra items in the left set:",
                "1",
                "Extra items in the right set:",
                "3",
                "Use -v to get more diff",
            ],
        ),
        ("==", "spam\neggs\n", "spam\nham\n", ["  spam", "- ham", "+ eggs"]),
        (
            "==",
            "a" * 60 + "x",
            "a" * 60 + "y",
            [
                "Skipping 50 identical leading characters in diff, use -v to show",
                "- aaaaaaaaaay",
                "?           ^",
                "+ aaaaaaaaaax",
                "?           ^",
            ],
        ),
        (
            "==",
            " ",
            "  ",
            [
                "Strings contain only whitespace, escaping them using repr()",
                "- '  '",
                "?   -",
                "+ ' '",
            ],
        ),
        ("==", b"ab", b"ac", ["At index 1 diff: b'b' != b'c'", "Use -v to get more diff"]),
        (
            "not in",
            "ham",
            "spam ham eggs",
            ["'ham' is contained here:", "  spam ham eggs", "?      +++"],
        ),
        ("<", {1}, {1}, ["Both sets are equal"]),
        ("in", 1, [2], []),
    ],
)
def test_explanations_read_like_pytests(op, left, right, lines):
    assert ne.explain(op, left, right) == lines


def test_dataclass_explanations_drill_into_fields(no_ci):
    @dataclasses.dataclass
    class Point:
        x: int
        y: int

    assert ne.explain("==", Point(1, 2), Point(1, 3)) == [
        "Omitting 1 identical items, use -vv to show",
        "Differing attributes:",
        "['y']",
        "",
        "Drill down into differing attribute y:",
        "  y: 2 != 3",
    ]


def test_saferepr_bounds_and_survives_broken_reprs():
    class Broken:
        def __repr__(self):
            raise RuntimeError("nope")

    assert ne.saferepr(list(range(10))) == "[0, 1, 2, 3, 4, 5, ...]"
    assert ne.saferepr({"b": 1, "a": 2}) == "{'b': 1, 'a': 2}"  # its own order, as pytest's
    assert ne.saferepr(dict.fromkeys("abcdef", 0)) == "{'a': 0, 'b': 0, 'c': 0, 'd': 0, ...}"
    assert ne.saferepr("x" * 50, maxsize=11) == "'xxx...xxx'"
    assert ne.saferepr(Broken()).startswith(
        "<[RuntimeError('nope') raised in repr()] Broken object at 0x"
    )


# ------------------------------------------------------------------ builtins


def builtins_for(config=None, **classes):
    return ne.builtin_resources(config or Config(), **classes)


def test_tmp_path_layout_under_a_given_basetemp(tmp_path):
    config = Config({"basetemp": str(tmp_path / "base")})
    resources = builtins_for(config)
    seen = []
    m = module("t.py")

    def test(tmp_path, tmpdir, tmp_path_factory):
        seen.append(
            (tmp_path, tmpdir, tmp_path_factory.mktemp("data"), tmp_path_factory.getbasetemp())
        )
        (tmp_path / "file.txt").write_text("hi")

    cases = [
        case(m, test, name="test_x[a b/c]", closure=("tmp_path_factory", "tmp_path", "tmpdir")),
        case(m, test, name="test_x[a b/c]", closure=("tmp_path_factory", "tmp_path", "tmpdir")),
        case(
            m, test, name="test_" + "long" * 20, closure=("tmp_path_factory", "tmp_path", "tmpdir")
        ),
    ]
    sink, _ = run(cases, builtins=resources)
    assert set(outcomes(sink).values()) == {"passed"}
    base = (tmp_path / "base").resolve()
    assert [s[3] for s in seen] == [base, base, base]
    assert [s[0].name for s in seen] == [
        "test_x_a_b_c_0",
        "test_x_a_b_c_1",
        ("test_" + "long" * 20)[:30] + "0",
    ]
    assert all(s[0].parent == base for s in seen)
    assert [s[2].name for s in seen] == ["data0", "data1", "data2"]
    assert all(str(s[1]) == str(s[0]) and isinstance(s[1], ne.LocalPath) for s in seen)
    assert (base / "test_x_a_b_c_0" / "file.txt").read_text() == "hi"
    assert (base / ".aye-run").read_text() == ne._RUN


def test_tmp_path_without_basetemp_makes_one_numbered_run_directory(tmp_path, monkeypatch):
    monkeypatch.setenv("PYTEST_DEBUG_TEMPROOT", str(tmp_path))
    root = tmp_path / f"pytest-of-{ne._user()}"
    root.mkdir()
    for number in range(5):
        (root / f"pytest-{number}").mkdir()
    (root / "pytest-1" / ".lock").write_text("123")  # a live session's: kept
    factory = ne.TempPathFactory.from_config(Config())
    base = factory.getbasetemp()
    assert base == (root / "pytest-5").resolve()
    assert (base / ".lock").read_text() == str(os.getpid())
    assert ne.TempPathFactory.from_config(Config()).getbasetemp() == base  # the run's, shared
    kept = sorted(p.name for p in root.glob("pytest-[0-9]*"))
    assert kept == ["pytest-1", "pytest-3", "pytest-4", "pytest-5"]
    assert factory.mktemp("x").name == "x0"
    with pytest.raises(ValueError, match="is not a normalized and relative path"):
        factory.mktemp("../escape")


def test_local_path_basics(tmp_path):
    path = ne.LocalPath(tmp_path)
    file = path.join("a", "b.txt")
    file.write("text", ensure=True)
    assert file.read() == "text" and file.check(file=1) and not file.check(dir=1)
    assert file.basename == "b.txt" and file.purebasename == "b" and file.ext == ".txt"
    assert file.dirpath() == path / "a"
    assert file.relto(path) == os.path.join("a", "b.txt")
    assert [p.basename for p in path.join("a").listdir()] == ["b.txt"]
    assert path.ensure("c", dir=True).isdir()
    assert os.fspath(file) == str(file)


class FakeMonkeyPatch:
    undone: ClassVar[list] = []

    def __init__(self):
        self.saved = []

    def setenv(self, name, value):
        self.saved.append((name, os.environ.get(name)))
        os.environ[name] = value

    def undo(self):
        for name, value in reversed(self.saved):
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
        FakeMonkeyPatch.undone.append(self)


def test_monkeypatch_is_undone_at_teardown():
    resources = builtins_for(monkeypatch_class=FakeMonkeyPatch)
    seen = []

    def test(monkeypatch):
        monkeypatch.setenv("AYE_NATIVE_TEST_VAR", "set")
        seen.append((monkeypatch, os.environ["AYE_NATIVE_TEST_VAR"]))

    sink, _ = run([case(module("t.py"), test)], builtins=resources)
    assert only(sink)["outcome"] == "passed"
    assert seen[0][1] == "set"
    assert FakeMonkeyPatch.undone[-1] is seen[0][0]
    assert "AYE_NATIVE_TEST_VAR" not in os.environ


def test_monkeypatch_and_recwarn_need_their_classes():
    assert "monkeypatch" not in builtins_for() and "recwarn" not in builtins_for()


def test_capsys_reads_and_resets_and_leaves_the_rest_to_the_report():
    resources = builtins_for()
    reads = []

    def test(capsys):
        print("first")
        sys.stderr.write("oops\n")
        reads.append(capsys.readouterr())
        print("second")
        reads.append(capsys.readouterr())
        print("unread")

    sink, _ = run([case(module("t.py"), test)], builtins=resources)
    event = only(sink)
    assert event["outcome"] == "passed"
    assert reads[0] == ("first\n", "oops\n") and reads[0].out == "first\n"
    assert reads[1] == ("second\n", "")
    assert event["output"]["stdout"] == "unread\n"


def test_output_is_captured_per_test_and_stdin_refuses_reads():
    def test():
        print("hello from the test")
        sys.stdin.read()

    sink, _ = run([case(module("t.py"), test)])
    event = only(sink)
    assert event["output"]["stdout"] == "hello from the test\n"
    assert event["outcome"] == "failed"
    assert "reading from stdin while output is captured" in event["failures"][0]["message"]


def test_capsys_disabled_writes_past_the_capture(capsys):
    resources = builtins_for()

    def test(capsys):
        with capsys.disabled():
            print("straight through")
        print("captured")

    sink, _ = run([case(module("t.py"), test)], builtins=resources)
    assert only(sink)["output"]["stdout"] == "captured\n"
    assert "straight through" in capsys.readouterr().out


def test_capsysbinary_reads_bytes():
    reads = []

    def test(capsysbinary):
        sys.stdout.buffer.write(b"\x00raw")
        print("text")
        reads.append(capsysbinary.readouterr())

    run([case(module("t.py"), test)], builtins=builtins_for())
    assert reads == [(b"\x00rawtext\n", b"")]


def test_capfd_captures_the_file_descriptors():
    reads = []

    def test(capfd):
        os.write(1, b"fd one\n")
        print("python")
        os.write(2, b"fd two\n")
        reads.append(capfd.readouterr())

    sink, _ = run([case(module("t.py"), test)], builtins=builtins_for())
    assert only(sink)["outcome"] == "passed"
    assert reads == [("fd one\npython\n", "fd two\n")]


def test_two_capture_resources_in_one_test_are_a_setup_error():
    sink, _ = run(
        [
            case(
                module("t.py"),
                lambda capsys, capfd: None,
                name="test",
                argnames=("capsys", "capfd"),
            )
        ],
        builtins=builtins_for(),
    )
    event = only(sink)
    assert event["outcome"] == "error"
    assert event["failures"][0]["message"] == "cannot use capfd and capsys at the same time"


def test_caplog_sees_the_phase_and_restores_levels():
    resources = builtins_for()
    seen = {}
    logger = logging.getLogger("aye.native.test")

    def test(caplog):
        logger.info("hidden")
        logger.warning("shown %s", 1)
        seen["before"] = list(caplog.messages)
        caplog.set_level(logging.INFO, logger="aye.native.test")
        logger.info("now visible")
        seen["after"] = caplog.record_tuples[-1]
        seen["setup"] = caplog.get_records("setup")
        with caplog.at_level(logging.ERROR):
            pass

    sink, _ = run([case(module("t.py"), test)], builtins=resources)
    event = only(sink)
    assert event["outcome"] == "passed"
    assert seen["before"] == ["shown 1"]
    assert seen["after"] == ("aye.native.test", logging.INFO, "now visible")
    assert seen["setup"] == []
    assert logger.level == logging.NOTSET
    assert "WARNING  aye.native.test:test_native_engine.py" in event["output"]["log"]
    assert "shown 1" in event["output"]["log"]


def test_a_log_call_with_bad_arguments_fails_the_test():
    def test():
        logging.getLogger("aye.native.bad").warning("%s %s", "one")  # noqa: PLE1206 - on purpose

    sink, _ = run([case(module("t.py"), test)])
    assert only(sink)["outcome"] == "failed"


def test_recwarn_records_with_the_given_class():
    class Recorder(warnings.catch_warnings):
        def __init__(self, *, _ispytest=False):
            super().__init__(record=True)

        def __enter__(self):
            self.list = super().__enter__()
            warnings.simplefilter("always")
            return self

    seen = []

    def test(recwarn):
        warnings.warn("careful", UserWarning, stacklevel=1)
        seen.append([str(w.message) for w in recwarn.list])

    sink, _ = run(
        [case(module("t.py"), test)], builtins=builtins_for(warnings_recorder_class=Recorder)
    )
    assert only(sink)["outcome"] == "passed"
    assert seen == [["careful"]]


def test_record_property_and_testsuite_property():
    def test(record_property, record_testsuite_property):
        record_property("size", 3)
        record_testsuite_property("suite", "nightly")

    sink, _ = run([case(module("t.py"), test)], builtins=builtins_for())
    assert only(sink)["properties"] == [["size", "3"]]
    assert next(e for e in sink.events if e["event"] == "run_property")["value"] == "nightly"


def test_subtests_report_and_fail_the_test():
    def test(subtests):
        for i in range(3):
            with subtests.test(msg="loop", i=i):
                assert i != 1

    sink, _ = run([case(module("t.py"), test)], builtins=builtins_for())
    subs = [e for e in sink.events if e["event"] == "sub_outcome"]
    assert [(s["name"], s["outcome"]) for s in subs] == [
        ("loop (i=0)", "passed"),
        ("loop (i=1)", "failed"),
        ("loop (i=2)", "passed"),
    ]
    assert only(sink)["outcome"] == "failed"
    assert only(sink)["failures"][0]["message"] == "contains 1 failed subtest"


def test_user_definitions_override_builtins():
    resources = builtins_for()
    m = module("t.py", fixture("tmp_path", lambda: "mine"))
    seen = []

    def test(tmp_path):
        seen.append(tmp_path)

    run(
        [case(m, test)],
        builtins=resources,
    )
    assert seen == ["mine"]


# ------------------------------------------------------------------ warnings


def test_filterwarnings_tags_apply_outermost_last():
    def test():
        warnings.warn("loud", UserWarning, stacklevel=1)

    m = module("t.py")
    error_closest = [Tag("filterwarnings", ("error",)), Tag("filterwarnings", ("ignore",))]
    ignore_closest = [Tag("filterwarnings", ("ignore",)), Tag("filterwarnings", ("error",))]
    sink, _ = run(
        [case(m, test, name="a", tags=error_closest), case(m, test, name="b", tags=ignore_closest)]
    )
    # The outer (later) tag wins.
    assert outcomes(sink) == {"t.py::a": "passed", "t.py::b": "failed"}


def test_ini_filterwarnings_turn_warnings_into_failures():
    def test():
        warnings.warn("deprecated thing", DeprecationWarning, stacklevel=1)

    config = Config(ini={"filterwarnings": ["error::DeprecationWarning"]})
    sink, _ = run([case(module("t.py"), test)], config=config)
    assert only(sink)["outcome"] == "failed"
    assert only(sink)["failures"][0]["exception"] == "DeprecationWarning"


def test_returning_a_value_warns_with_the_given_class():
    class ReturnWarning(UserWarning):
        pass

    m = module("t.py")
    sink, _ = run(
        [case(m, lambda: 1, name="test", argnames=(), tags=[Tag("filterwarnings", ("error",))])],
        return_warning=ReturnWarning,
    )
    assert only(sink)["failures"][0]["exception"] == "ReturnWarning"


# ------------------------------------------------------------------ hooks, env, stopping


def test_hooks_run_around_each_phase():
    log = []
    hooks = Hooks(
        setup=[lambda c: log.append(("setup", c.id))],
        call=[lambda c: log.append(("call", c.id))],
        teardown=[lambda c: log.append(("teardown", c.id))],
    )

    def fx():
        log.append("fixture")
        yield
        log.append("fixture down")

    m = module("t.py", fixture("fx", fx))
    the_case = case(m, lambda fx: log.append("body"), name="test", argnames=("fx",))
    run([the_case], hooks=hooks)
    assert log == [
        ("setup", "t.py::test"), "fixture", ("call", "t.py::test"), "body",
        ("teardown", "t.py::test"), "fixture down",
    ]  # fmt: skip


def test_the_current_test_is_in_the_environment():
    seen = []

    def fx():
        seen.append(os.environ["AYE_CURRENT"])
        yield
        seen.append(os.environ["AYE_CURRENT"])

    m = module("t.py", fixture("fx", fx))
    run(
        [case(m, lambda fx: seen.append(os.environ["AYE_CURRENT"]), name="test", argnames=("fx",))],
        current_test_env="AYE_CURRENT",
    )
    assert seen == ["t.py::test (setup)", "t.py::test (call)", "t.py::test (teardown)"]
    assert "AYE_CURRENT" not in os.environ


def test_maxfail_stops_and_the_last_test_closes_every_scope():
    log = []

    def mod():
        yield
        log.append("module down")

    m = module("t.py", fixture("mod", mod, "module"))

    def bad(mod):
        log.append("bad")
        raise ValueError

    cases = [case(m, bad, name=f"test_{i}") for i in range(4)]
    sink, status = run(cases, maxfail=2)
    assert status == 1
    assert list(outcomes(sink).values()) == ["failed", "failed"]
    assert log == ["bad", "bad", "module down"]
    assert not [e for e in sink.events if e["event"] == "diagnostic"]


def test_an_exit_stops_the_run_with_its_status():
    log = []

    def mod():
        yield
        log.append("module down")

    m = module("t.py", fixture("mod", mod, "module"))

    def stop(mod):
        raise Exit("enough", returncode=3)

    cases = [
        case(m, lambda mod: None, name="test_0"),
        case(m, stop, name="test_1"),
        case(m, lambda mod: None, name="test_2"),
    ]
    sink, status = run(cases)
    assert status == 3
    assert outcomes(sink) == {"t.py::test_0": "passed", "t.py::test_1": "error"}
    assert log == ["module down"]
    (diagnostic,) = [e for e in sink.events if e["event"] == "diagnostic"]
    assert diagnostic["code"] == "batch-stopped"
    assert "Exit: enough" in diagnostic["message"]
    assert "1 of 3 tests in the batch never started: t.py::test_2" in diagnostic["message"]


def test_a_keyboard_interrupt_stops_the_run():
    def test():
        raise KeyboardInterrupt

    m = module("t.py")
    sink, status = run([case(m, test), case(m, lambda: None, name="later", argnames=())])
    assert status == 2
    assert outcomes(sink) == {"t.py::test": "error"}


def test_session_shouldfail_stops_the_run():
    def test(request):
        request.session.shouldfail = "conftest says stop"

    m = module("t.py")
    sink, status = run([case(m, test), case(m, lambda: None, name="later", argnames=())])
    assert status == 1
    assert list(outcomes(sink)) == ["t.py::test"]


def test_the_context_and_cleanup_services_follow_the_test():
    services = aye_engine.default_services()
    context = services.get("context")
    seen = []

    def test():
        seen.append(context.test)
        services.get("cleanup").add(lambda: seen.append("cleaned"))

    engine = ne.Engine(aye_engine.Paths("/repo"), services, POLICY)
    sink = aye_engine.ListSink()
    engine.run([case(module("t.py"), test)], sink)
    assert seen == ["t.py::test", "cleaned"]
    assert context.test is None


# ------------------------------------------------------------------ unittest


class Calls(unittest.TestCase):
    __test__ = False  # run by the engine below, not by pytest
    log: ClassVar[list] = []

    @classmethod
    def setUpClass(cls):
        cls.log.append("setUpClass")

    @classmethod
    def tearDownClass(cls):
        cls.log.append("tearDownClass")

    def setUp(self):
        self.log.append(f"setUp {self._testMethodName}")

    def test_pass(self):
        pass

    def test_fail(self):
        self.assertEqual(1, 2)

    def test_error(self):
        raise KeyError("k")

    @unittest.skip("not now")
    def test_skip(self):
        pass

    @unittest.expectedFailure
    def test_expected(self):
        self.fail("as expected")

    @unittest.expectedFailure
    def test_unexpected(self):
        pass

    def test_subtests(self):
        for i in range(3):
            with self.subTest(i=i):
                self.assertNotEqual(i, 1)


def unittest_case(cls, name, node=None):
    m = node or child(module("t.py"), f"t.py::{cls.__name__}", "class", cls)
    return Case(
        f"{m.id}::{name}", m, getattr(cls, name), cls=cls, kind="unittest", location=("t.py", 1)
    )


def test_unittest_cases_run_through_testcase_run():
    Calls.log.clear()
    klass = child(module("t.py"), "t.py::Calls", "class", Calls)
    names = [
        "test_pass",
        "test_fail",
        "test_error",
        "test_skip",
        "test_expected",
        "test_unexpected",
        "test_subtests",
    ]
    sink, status = run([unittest_case(Calls, name, klass) for name in names])
    assert status == 1
    assert outcomes(sink) == {
        "t.py::Calls::test_pass": "passed",
        "t.py::Calls::test_fail": "failed",
        "t.py::Calls::test_error": "failed",
        "t.py::Calls::test_skip": "skipped",
        "t.py::Calls::test_expected": "expected_failure",
        "t.py::Calls::test_unexpected": "failed",
        "t.py::Calls::test_subtests": "failed",
    }
    events = finished(sink)
    assert events["t.py::Calls::test_skip"]["reason"] == "not now"
    assert events["t.py::Calls::test_fail"]["failures"][0]["message"] == "AssertionError: 1 != 2"
    assert all(
        "unittest" not in f["path"]
        for f in events["t.py::Calls::test_fail"]["failures"][0]["frames"]
    )
    assert Calls.log[0] == "setUpClass" and Calls.log[-1] == "tearDownClass"
    assert Calls.log.count("setUpClass") == 1
    subs = [(e["name"], e["outcome"]) for e in sink.events if e["event"] == "sub_outcome"]
    assert subs == [("(i=0)", "passed"), ("(i=1)", "failed"), ("(i=2)", "passed")]


def test_a_failing_set_up_class_errors_every_test_of_the_class():
    calls = []

    class Broken(unittest.TestCase):
        @classmethod
        def setUpClass(cls):
            calls.append(1)
            raise RuntimeError("class setup")

        def test_a(self):
            pass

        def test_b(self):
            pass

    klass = child(module("t.py"), "t.py::Broken", "class", Broken)
    sink, _ = run([unittest_case(Broken, "test_a", klass), unittest_case(Broken, "test_b", klass)])
    assert set(outcomes(sink).values()) == {"error"}
    assert calls == [1]


def test_a_set_up_error_is_a_failure_and_a_skipped_class_skips_its_tests():
    class SetUpFails(unittest.TestCase):
        def setUp(self):
            raise RuntimeError("setUp")

        def test_a(self):
            pass

    @unittest.skip("whole class")
    class Skipped_(unittest.TestCase):
        def test_a(self):
            pass

    sink, _ = run([unittest_case(SetUpFails, "test_a"), unittest_case(Skipped_, "test_a")])
    assert list(outcomes(sink).values()) == ["failed", "skipped"]
    assert finished(sink)["t.py::Skipped_::test_a"]["reason"] == "whole class"


def test_unittest_cases_honour_tags_and_autouse_resources():
    log = []

    class Tagged(unittest.TestCase):
        def test_a(self):
            log.append("ran")

    m = module("t.py", fixture("auto", lambda: log.append("auto"), autouse=True))
    klass = child(m, "t.py::Tagged", "class", Tagged)
    the_case = unittest_case(Tagged, "test_a", klass)
    the_case.closure = ("auto",)
    sink, _ = run([the_case])
    assert only(sink)["outcome"] == "passed"
    assert log == ["auto", "ran"]
    the_case.tags = [Tag("skip", (), {"reason": "tagged"})]
    sink, _ = run([the_case])
    assert only(sink)["outcome"] == "skipped"


def test_an_unknown_kind_fails_clearly():
    the_case = case(module("t.py"), lambda: None, name="test", argnames=(), kind="doctest")
    sink, _ = run([the_case])
    assert (
        only(sink)["failures"][0]["message"]
        == "Failed: aye's engine has no runner for tests of kind 'doctest'"
    )


def test_items_and_nodes_answer_the_common_questions(tmp_path):
    seen = {}

    def test(request):
        item = request.node
        item.add_marker("dynamic")
        seen["keywords"] = "dynamic" in item.keywords and "test_it[1]" in item.keywords
        seen["chain"] = [n.name for n in item.listchain()]
        seen["callspec"] = item.callspec.params
        seen["originalname"] = item.originalname
        seen["markers"] = [m.name for m in item.iter_markers()]

    m = module("tests/t.py", tags=[Tag("module_mark")])
    the_case = case(
        m, test, name="test_it[1]", call_params={"x": 1}, tags=[Tag("own"), Tag("module_mark")]
    )
    sink, _ = run([the_case])
    assert only(sink)["outcome"] == "passed"
    assert seen == {
        "keywords": True,
        "chain": ["", "t.py", "test_it[1]"],
        "callspec": {"x": 1},
        "originalname": "test_it",
        "markers": ["dynamic", "own", "module_mark"],
    }
    assert Path(__file__).name == "test_native_engine.py"


class FrontItem:
    """A front-end's item: what its hooks receive."""

    def __init__(self, name, marks):
        self.name = name
        self.own_markers = list(marks)
        self.user_properties = []
        self.funcargs = {}

    def iter_markers(self, name=None):
        return (m for m in self.own_markers if name is None or m.name == name)

    def add_marker(self, marker, append=True):
        self.own_markers.append(marker)


def test_the_front_ends_item_is_request_node_and_its_marks_count():
    seen = {}

    def hook(the_case):
        item = the_case.extra["item"]
        item.add_marker(Tag("xfail", (), {"reason": "a hook said so"}))

    def fx(request):
        seen["node"] = request.node
        request.node.user_properties.append(("from", "fixture"))
        return 1

    def test(fx):
        seen["funcargs"] = dict(item.funcargs)
        raise ValueError("expected")

    m = module("t.py", fixture("fx", fx))
    item = FrontItem("test", [Tag("own")])
    the_case = case(m, test, extra={"item": item})
    sink, _ = run([the_case], hooks=Hooks(setup=[hook]))
    event = only(sink)
    assert event["outcome"] == "expected_failure"
    assert event["reason"] == "a hook said so"
    assert seen["node"] is item
    assert seen["funcargs"] == {"fx": 1}
    assert event["properties"] == [["from", "fixture"]]


def test_scope_nodes_read_what_they_lack_from_the_front_ends_collectors():
    seen = {}

    class TestK:
        def test_a(self, cls_fx):
            pass

    class Collector:
        def __init__(self, obj, parent, extra):
            self.obj, self.parent, self.extra = obj, parent, extra

    session = Collector(None, None, "session's")
    module_collector = Collector(sys.modules[__name__], session, "module's")
    class_collector = Collector(TestK, module_collector, "class's")
    item = FrontItem("test_a", [])
    item.parent = class_collector

    def cls_fx(request):
        node = request.node
        seen["class"] = (node.name, node.cls, node.module, node.extra)
        seen["module"] = node.parent.extra
        node.addfinalizer(lambda: seen.setdefault("closed", True))

    m = module("t.py")
    klass = child(m, "t.py::TestK", "class", TestK, fixture("cls_fx", cls_fx, "class"))
    sink, _ = run([case(klass, TestK.test_a, cls=TestK, extra={"item": item})])
    assert only(sink)["outcome"] == "passed"
    assert seen == {
        "class": ("TestK", TestK, sys.modules[__name__], "class's"),
        "module": "module's",
        "closed": True,
    }


# ------------------------------------------------------------------ tags read once


class CountingItem(FrontItem):
    """A front-end's item that counts how often its marks are read."""

    def __init__(self, name, marks):
        super().__init__(name, marks)
        self.reads = 0

    def iter_markers(self, name=None):
        self.reads += 1
        return super().iter_markers(name)


def test_a_cases_tags_are_read_once_unless_a_tag_is_added():
    item = CountingItem("test", [Tag("skipif", (False,), {"reason": "never"})])
    m = module("t.py")
    sink, _ = run([case(m, lambda: None, name="test", argnames=(), extra={"item": item})])
    assert only(sink)["outcome"] == "passed"
    assert item.reads == 1

    def adds_xfail(request):
        request.applymarker(Tag("xfail", (), {"reason": "added"}))

    def fails(fx):
        raise ValueError("as expected")

    added = CountingItem("test", [])
    m = module("t.py", fixture("fx", adds_xfail))
    sink, _ = run([case(m, fails, name="test", extra={"item": added})])
    assert only(sink)["outcome"] == "expected_failure"
    assert added.reads == 2


def test_an_engine_told_tags_changed_reads_them_again():
    """A front-end whose parents' marks change says so (``tags_changed``)."""
    marks = []

    class Item(FrontItem):
        def iter_markers(self, name=None):
            return iter([*self.own_markers, *marks])

    engine = engine_for()

    def setup_hook(the_case):
        marks.append(Tag("xfail", (), {"reason": "from a parent"}))
        engine.tags_changed()

    engine.hooks = Hooks(setup=[setup_hook])

    def fails():
        raise ValueError("expected")

    sink = aye_engine.ListSink()
    m = module("t.py")
    engine.run([case(m, fails, name="test", extra={"item": Item("test", [])})], sink)
    assert only(sink)["outcome"] == "expected_failure"
