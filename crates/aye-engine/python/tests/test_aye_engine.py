import collections
import dataclasses
import datetime
import decimal
import enum
import fractions
import hashlib
import json
import math
import os
import sys
import time
import traceback
import unittest
from unittest import mock

import aye_engine as engine
import aye_rewrite
import pytest
from aye_engine import Batch, EngineExecutor, Lifetime, Outcome, Phase, Test


def events_of(sink, kind):
    return [e for e in sink.events if e["event"] == kind]


class Recorder:
    def __init__(self):
        self.calls = []

    def __call__(self, name):
        return lambda: self.calls.append(name)


class FakeExecutor(EngineExecutor):
    adapter = "fake"

    def __init__(self, root, tests, services=None):
        super().__init__(root, services or engine.default_services())
        self.tests = tests

    def load(self, batch):
        wanted = set(batch.ids)
        return [t for t in self.tests if t.id in wanted]


def run(tests, ids=None, policy=None, root="/repo", context=None):
    sink = engine.ListSink(batch=0)
    executor = FakeExecutor(root, tests)
    batch = Batch(
        {
            "id": 0,
            "adapter": "fake",
            "ids": ids or [t.id for t in tests],
            "policy": policy or {},
            "context": context,
        }
    )
    status = executor.run(batch, sink)
    return sink, status


def outcomes(sink):
    return {e["id"]: e["outcome"] for e in events_of(sink, "test_finished")}


def test_outcomes_follow_what_the_body_raises():
    def fail():
        assert 1 == 2

    def boom():
        raise ValueError("boom")

    def skip():
        raise engine.Skip("not today")

    sink, status = run(
        [
            Test("t.py::test_pass", lambda: None),
            Test("t.py::test_fail", fail),
            Test("t.py::test_error", boom),
            Test("t.py::test_skip", skip),
            Test("t.py::test_xfail", fail, expected="known bug"),
            Test("t.py::test_xpass", lambda: None, expected="known bug"),
        ]
    )
    assert outcomes(sink) == {
        "t.py::test_pass": "passed",
        "t.py::test_fail": "failed",
        "t.py::test_error": "error",
        "t.py::test_skip": "skipped",
        "t.py::test_xfail": "expected_failure",
        "t.py::test_xpass": "unexpected_pass",
    }
    assert status == 1
    finished = {e["id"]: e for e in events_of(sink, "test_finished")}
    error = finished["t.py::test_error"]["failures"][0]
    assert error["exception"] == "ValueError"
    assert error["message"] == "ValueError: boom"
    assert error["phase"] == "test"
    assert finished["t.py::test_skip"]["reason"] == "not today"


def test_lifetimes_open_and_close_as_the_order_requires():
    calls = Recorder()
    module = Lifetime("m", "module", calls("setup m"), calls("teardown m"))
    a = Lifetime("m::A", "class", calls("setup A"), calls("teardown A"))
    b = Lifetime("m::B", "class", calls("setup B"), calls("teardown B"))
    tests = [
        Test("m::A::t1", calls("A.t1"), lifetimes=[module, a]),
        Test("m::A::t2", calls("A.t2"), lifetimes=[module, a]),
        Test("m::B::t1", calls("B.t1"), lifetimes=[module, b]),
        Test("m::t3", calls("t3"), lifetimes=[module]),
    ]
    sink, _ = run(tests)
    assert calls.calls == [
        "setup m",
        "setup A",
        "A.t1",
        "A.t2",
        "teardown A",
        "setup B",
        "B.t1",
        "teardown B",
        "t3",
        "teardown m",
    ]
    kinds = [
        (e["event"], e.get("lifetime") or e.get("id"))
        for e in sink.events
        if "lifetime" in e["event"]
    ]
    assert kinds == [
        ("lifetime_started", "m"),
        ("lifetime_started", "m::A"),
        ("lifetime_finished", "m::A"),
        ("lifetime_started", "m::B"),
        ("lifetime_finished", "m::B"),
        ("lifetime_finished", "m"),
    ]


def test_a_failed_lifetime_is_reported_once_and_blocks_its_tests():
    def broken():
        raise RuntimeError("database is down")

    calls = Recorder()
    cls = Lifetime("m::A", "class", broken, calls("teardown A"))
    tests = [
        Test("m::A::t1", calls("t1"), lifetimes=[cls]),
        Test("m::A::t2", calls("t2"), lifetimes=[cls]),
        Test("m::t3", calls("t3")),
    ]
    sink, _ = run(tests)
    assert calls.calls == ["t3"], "neither the tests nor the teardown of a failed setup run"
    finished = {e["id"]: e for e in events_of(sink, "test_finished")}
    assert finished["m::A::t1"]["outcome"] == "error"
    assert finished["m::A::t1"]["cause"] == "m::A"
    assert "failures" not in finished["m::A::t1"]
    (closed,) = events_of(sink, "lifetime_finished")
    assert closed["failure"]["message"] == "RuntimeError: database is down"
    assert closed["failure"]["phase"] == "setup"


def test_a_skipped_lifetime_skips_its_tests():
    def skip():
        raise engine.Skip("no database")

    cls = Lifetime("m::A", "class", skip)
    sink, _ = run([Test("m::A::t1", lambda: None, lifetimes=[cls])])
    (finished,) = events_of(sink, "test_finished")
    assert finished["outcome"] == "skipped"
    assert finished["reason"] == "no database"


def test_teardown_failure_after_a_pass_is_one_error_with_its_phase():
    def teardown():
        raise OSError("disk full")

    sink, _ = run([Test("t.py::test_x", lambda: None, teardown=teardown)])
    (finished,) = events_of(sink, "test_finished")
    assert finished["outcome"] == "error"
    assert [f["phase"] for f in finished["failures"]] == ["teardown"]


def test_cleanups_run_last_in_first_out_even_when_setup_fails():
    services = engine.default_services()
    cleanup = services.get(engine.Services.CLEANUP)
    order = []

    def setup():
        cleanup.add(order.append, "first")
        cleanup.add(order.append, "second")
        raise RuntimeError("halfway")

    sink = engine.ListSink()
    executor = FakeExecutor(
        "/repo", [Test("t.py::test_x", lambda: order.append("body"), setup=setup)], services
    )
    executor.run(Batch({"id": 0, "adapter": "fake", "ids": ["t.py::test_x"]}), sink)
    assert order == ["second", "first"]
    (finished,) = events_of(sink, "test_finished")
    assert finished["outcome"] == "error"
    assert finished["failures"][0]["phase"] == "setup"


def test_maxfail_stops_the_batch():
    def fail():
        raise AssertionError

    tests = [Test(f"t.py::test_{i}", fail) for i in range(5)]
    sink, _ = run(tests, policy={"maxfail": 2})
    assert len(events_of(sink, "test_finished")) == 2


def test_services_are_looked_up_by_name_and_may_be_absent():
    services = engine.Services({"clock": object()})
    assert services.has("clock")
    assert services.get("network") is None
    assert services.get("network", "fallback") == "fallback"
    try:
        services.require("network")
    except engine.ServiceUnavailable as exc:
        assert "network" in str(exc)
    else:
        raise AssertionError("require must raise for an absent service")


def test_failures_keep_only_user_frames_relative_to_the_root(tmp_path):
    source = tmp_path / "test_mod.py"
    source.write_text("def check():\n    assert 1 == 2, 'nope'\n")
    namespace = {}
    exec(compile(source.read_text(), str(source), "exec"), namespace)
    try:
        namespace["check"]()
    except AssertionError as exc:
        failure = engine.Failure.from_exception(exc, engine.Paths(str(tmp_path)), phase=Phase.TEST)
    data = failure.to_dict()
    assert data["message"] == "nope"
    assert data["location"] == {"path": "test_mod.py", "line": 2}
    assert data["frames"][-1]["source"] == "assert 1 == 2, 'nope'"
    assert [f["path"] for f in data["frames"]][-1] == "test_mod.py"
    assert len(data["signature"]) == 16


def test_signatures_group_failures_that_differ_only_in_numbers():
    a = engine.signature("AssertionError", "assert 3 == 4", ("t.py", 2))
    b = engine.signature("AssertionError", "assert 7 == 9", ("t.py", 2))
    c = engine.signature("AssertionError", "assert 3 == 4", ("t.py", 9))
    assert a == b != c


def test_signatures_never_fail_on_lone_surrogates():
    # Bytes Python could not decode (a file name, `surrogateescape`) reach the
    # exception name, the message and the location; a strict encode raised.
    surrogate = "caf\udce9"
    raw = engine.signature(f"E{surrogate}", f"{surrogate} 3", (f"{surrogate}.py", 3))
    clean = engine.signature("Ecaf\ufffd", "caf\ufffd 3", ("caf\ufffd.py", 3))
    assert raw == clean
    assert len(raw) == 16
    # Through a failure, too: the record and its signature are valid UTF-8.
    data = engine.Failure(
        surrogate, exception=surrogate, location=(surrogate, 1), frames=[]
    ).to_dict()
    json.dumps(data, ensure_ascii=False).encode("utf-8")
    assert data["signature"] == engine.signature("caf\ufffd", "caf\ufffd", ("caf\ufffd", 1))


def test_text_is_bounded():
    text, cut = engine.bound("x" * 10_000, 100)
    assert cut and len(text) <= 100 and text.startswith("x")
    tail, cut = engine.bound_tail("y" * 10_000 + "END", 100)
    assert cut and len(tail) <= 100 and tail.endswith("END")
    tiny, cut = engine.bound_tail("abcdefgh", 3)
    assert (tiny, cut) == ("fgh", True)
    assert engine.bound("short") == ("short", False)


def test_worst_outcome_wins():
    assert engine.worst([Outcome.PASSED, Outcome.FAILED, Outcome.SKIPPED]) is Outcome.FAILED
    assert engine.worst([]) is Outcome.PASSED
    assert Outcome.ERROR.is_bad and not Outcome.EXPECTED_FAILURE.is_bad


def test_sys_exit_in_a_test_is_reported_on_the_test_and_the_batch_goes_on():
    def exits():
        sys.exit(3)

    sink, _ = run([Test("t.py::test_exit", exits), Test("t.py::test_after", lambda: None)])
    assert outcomes(sink) == {"t.py::test_exit": "error", "t.py::test_after": "passed"}


def _interrupt():
    raise KeyboardInterrupt


def test_a_keyboard_interrupt_finishes_the_test_as_an_error_and_propagates():
    for phase in ("setup", "test", "teardown"):
        calls = Recorder()
        services = engine.default_services()
        module = Lifetime("t.py", "module", calls("setup m"), calls("teardown m"))
        test = Test(
            "t.py::test_a",
            _interrupt if phase == "test" else calls("body"),
            lifetimes=[module],
            setup=_interrupt if phase == "setup" else None,
            teardown=_interrupt if phase == "teardown" else calls("teardown"),
        )
        after = Test("t.py::test_b", calls("never"), lifetimes=[module])
        sink = engine.ListSink(batch=0)
        executor = FakeExecutor("/repo", [test, after], services)
        batch = Batch({"id": 0, "adapter": "fake", "ids": ["t.py::test_a", "t.py::test_b"]})
        try:
            executor.run(batch, sink)
        except KeyboardInterrupt:
            pass
        else:
            raise AssertionError("the interrupt must propagate")
        (finished,) = events_of(sink, "test_finished")
        assert finished["outcome"] == "error", phase
        assert [f["phase"] for f in finished["failures"]] == [phase]
        assert finished["failures"][0]["exception"] == "KeyboardInterrupt"
        # The test's cleanup scope is closed and its lifetimes too.
        assert services.get(engine.Services.CLEANUP)._stack == []
        assert [e["lifetime"] for e in events_of(sink, "lifetime_finished")] == ["t.py"]
        assert "never" not in calls.calls
        assert "teardown m" in calls.calls
        # An interrupted body still gets its teardown.
        assert ("teardown" in calls.calls) == (phase == "test")


def test_an_interrupted_lifetime_setup_is_its_failure():
    calls = Recorder()
    module = Lifetime("t.py", "module", _interrupt, calls("teardown m"))
    sink = engine.ListSink(batch=0)
    executor = FakeExecutor("/repo", [Test("t.py::test_a", calls("body"), lifetimes=[module])])
    try:
        executor.run(Batch({"id": 0, "adapter": "fake", "ids": ["t.py::test_a"]}), sink)
    except KeyboardInterrupt:
        pass
    assert calls.calls == [], "neither the test nor the teardown of an unfinished setup runs"
    (closed,) = events_of(sink, "lifetime_finished")
    assert closed["failure"]["phase"] == "setup"
    assert closed["failure"]["exception"] == "KeyboardInterrupt"
    assert events_of(sink, "test_started") == []


def test_records_are_always_valid_json():
    surrogate = "caf\udce9"
    sink = engine.ListSink(batch=0)
    result = engine.Result("t.py::test_x")
    result.add(
        Outcome.FAILED,
        engine.Failure(
            f"FileNotFoundError: {surrogate}",
            phase=Phase.TEST,
            location=(f"{surrogate}.py", 3),
            frames=[{"path": f"{surrogate}.py", "line": 3, "source": surrogate}],
            detail=surrogate,
        ),
    )
    result.stdout = surrogate
    sink.test_finished(result)
    sink.diagnostic("warning", "x", surrogate, surrogate)
    sink.emit("custom", value=float("inf"), values=[1.0, float("nan")], nested={"t": float("-inf")})
    text = json.dumps(sink.events, ensure_ascii=False, allow_nan=False)
    text.encode("utf-8")  # strict: no lone surrogate left
    failure = sink.events[0]["failures"][0]
    assert failure["message"] == "FileNotFoundError: caf�"
    assert failure["location"]["path"] == "caf�.py"
    # The signature hashes the text sent, so it equals the clean text's.
    clean = engine.signature(None, "FileNotFoundError: caf�", ("caf�.py", 3))
    assert failure["signature"] == clean
    assert engine.signature(None, surrogate, None) == engine.signature(None, "caf�", None)
    custom = sink.events[-1]
    assert "value" not in custom
    assert custom["values"] == [1.0, None]
    assert custom["nested"] == {}


def _reference_sanitize(value):
    """``sanitize`` as it was first written, with ``isinstance`` throughout:
    the fast paths for exact types must give the same values."""
    if isinstance(value, str):
        return engine.clean(value)
    if isinstance(value, dict):
        return {
            (engine.clean(key) if isinstance(key, str) else key): _reference_sanitize(item)
            for key, item in value.items()
            if not (isinstance(item, float) and not math.isfinite(item))
        }
    if isinstance(value, (list, tuple)):
        return [
            None
            if isinstance(item, float) and not math.isfinite(item)
            else _reference_sanitize(item)
            for item in value
        ]
    if isinstance(value, float) and not math.isfinite(value):
        return None
    return value


def test_sanitize_takes_every_kind_of_value_as_it_did():
    class Text(str):
        pass

    class Number(float):
        pass

    class Level(enum.IntEnum):
        LOW = 1

    class Name(str, enum.Enum):
        A = "a\udce9"

    pair = collections.namedtuple("pair", "left right")
    surrogate = "caf\udce9"
    values = [
        surrogate,
        "plain",
        Text(surrogate),
        Text("plain"),
        1,
        True,
        None,
        2.5,
        float("nan"),
        Number("inf"),
        Number(1.5),
        Level.LOW,
        Name.A,
        b"bytes",
        (1, float("inf"), surrogate),
        pair(Text(surrogate), Number("nan")),
        [surrogate, [float("-inf"), {"k": float("nan")}], Level.LOW, Number("nan")],
        {surrogate: 1, "caf�": 2, Text("k"): Number("nan"), 3: float("inf"), None: True},
        collections.OrderedDict([("a", surrogate), ("b", (Name.A,))]),
        {"nested": {"deeper": [{"x": float("nan"), "y": Text(surrogate)}]}},
    ]
    for value in values:
        made, expected = engine.sanitize(value), _reference_sanitize(value)
        assert made == expected  # never NaN: non-finite numbers are gone
        assert type(made) is type(expected)
        if isinstance(expected, (dict, list)):
            assert list(map(type, made)) == list(map(type, expected))
            assert made is not value  # always a new object or list
    record = {"a": 1}
    assert engine.sanitize(record) is not record


def test_one_line_fields_are_bounded():
    failure = engine.Failure(
        "m" * 5000,
        exception="E" * 5000,
        frames=[{"path": "t.py", "line": 1, "function": "f" * 5000, "source": "s" * 5000}],
    ).to_dict()
    assert len(failure["message"]) == 1000 and "\n" not in failure["message"]
    assert len(failure["exception"]) == 1000
    assert len(failure["frames"][0]["source"]) == 1000
    assert len(failure["frames"][0]["function"]) == 1000
    sink = engine.ListSink()
    sink.sub_outcome("t.py::test_x", "n" * 5000, Outcome.PASSED)
    sink.diagnostic("info", "x", "", "d" * 50000)
    assert len(sink.events[0]["name"]) == 1000
    assert len(sink.events[1]["message"]) <= engine.MAX_TEXT
    assert engine.cut_line("short") == "short"


def test_a_test_that_breaks_the_clock_changes_no_duration(monkeypatch):
    # The engine bound perf_counter at import, as pytest binds its own: a
    # test replacing time.perf_counter (with NaN, inf, or anything) leaves
    # every duration, its own and the later tests', finite.
    def break_clock(value):
        return lambda: monkeypatch.setattr(time, "perf_counter", lambda: value)

    tests = [
        Test("t.py::test_nan", break_clock(float("nan")), lifetimes=[Lifetime("t.py", "module")]),
        Test("t.py::test_after_nan", lambda: None, lifetimes=[Lifetime("t.py", "module")]),
        Test("t.py::test_inf", break_clock(float("inf"))),
        Test("t.py::test_after_inf", lambda: None),
    ]
    sink, status = run(tests)
    assert status == 0
    for event in sink.events:
        assert math.isfinite(event["t_ms"])
        if event["event"] in ("test_finished", "lifetime_finished"):
            assert math.isfinite(event["duration_ms"]) and event["duration_ms"] >= 0, event
    assert len(events_of(sink, "test_finished")) == 4
    assert engine.perf_counter is not time.perf_counter


def test_reproduce_carries_the_arguments_after_the_double_dash():
    assert engine.reproduce("t.py::test_x") == {"command": ["aye", "t.py::test_x"]}
    assert engine.reproduce("t.py::test_x", []) == {"command": ["aye", "t.py::test_x"]}
    assert engine.reproduce("t.py::test_x[a b]", ["--backends=sqlite,postgres", "-v"]) == {
        "command": ["aye", "t.py::test_x[a b]", "--", "--backends=sqlite,postgres", "-v"]
    }


def test_a_failures_text_starts_at_the_users_code():
    # The frames before the user's code are the worker's and the engine's
    # (here the engine's own file): left out of ``detail`` as they are of
    # ``frames``.
    class Executor(FakeExecutor):
        internal = (engine.__file__,)

    def body():
        raise ValueError("boom")

    sink = engine.ListSink(batch=0)
    executor = Executor("/repo", [Test("t.py::test_boom", body)])
    executor.run(Batch({"id": 0, "adapter": "fake", "ids": ["t.py::test_boom"]}), sink)
    (finished,) = events_of(sink, "test_finished")
    detail = finished["failures"][0]["detail"]
    first_frame = detail.splitlines()[1]
    assert first_frame.startswith(f'  File "{__file__}"'), detail
    assert engine.__file__ not in detail, detail
    assert detail.endswith("ValueError: boom\n"), detail
    # Without ``paths``, Python's own text.
    try:
        body()
    except ValueError as exc:
        text = engine.format_exception(exc)
        assert text == "".join(traceback.format_exception(type(exc), exc, exc.__traceback__))


def test_reproduce_puts_ayes_own_options_before_the_id():
    # C26: aye's options that shaped the run (``-c FILE``), from the context
    # the binary adds them to, so that the rerun runs as the run did.
    assert engine.reproduce("t.py::test_x", ["-v"], ["-c", "alt.ini"]) == {
        "command": ["aye", "-c", "alt.ini", "t.py::test_x", "--", "-v"]
    }
    assert engine.reproduce("t.py::test_x", (), ["-c", "alt.ini"]) == {
        "command": ["aye", "-c", "alt.ini", "t.py::test_x"]
    }
    assert engine.rerun_args({"rerun_args": ["-c", "alt.ini"]}) == ["-c", "alt.ini"]
    assert engine.rerun_args({"rerun_args": ["-c", 3, "x.ini"]}) == ["-c", "x.ini"]
    assert engine.rerun_args({"rerun_args": "-c alt.ini"}) == []
    assert engine.rerun_args({}) == []
    assert engine.rerun_args(None) == []

    def fail():
        raise AssertionError

    sink, _ = run(
        [Test("t.py::test_fail", fail)],
        policy={"framework_args": ["-x"]},
        context={"rerun_args": ["-c", "alt.ini"]},
    )
    (finished,) = events_of(sink, "test_finished")
    assert finished["reproduce"] == {
        "command": ["aye", "-c", "alt.ini", "t.py::test_fail", "--", "-x"]
    }


def test_what_a_test_records_for_reports_is_carried():
    # C24: a test's properties and its body's duration on its
    # ``test_finished``, the run's properties as ``run_property`` events;
    # bounded as every text field is.
    sink = engine.ListSink(batch=0)
    result = engine.Result("t.py::test_x")
    result.add(Outcome.PASSED)
    result.properties = [("example_key", "1"), ("n" * 2000, "v" * 9000)]
    result.body_duration = 0.00125
    sink.test_finished(result)
    sink.run_property("ARCH", "PPC")
    finished, prop = sink.events
    assert finished["properties"][0] == ["example_key", "1"]
    name, value = finished["properties"][1]
    assert len(name) == engine.MAX_LINE
    assert len(value) <= engine.MAX_TEXT
    assert finished["body_duration_ms"] == 1.25
    assert prop == {
        "batch": 0,
        "t_ms": prop["t_ms"],
        "event": "run_property",
        "name": "ARCH",
        "value": "PPC",
    }
    # Neither is written when there is none.
    sink = engine.ListSink(batch=0)
    sink.test_finished(engine.Result("t.py::test_y"))
    (finished,) = sink.events
    assert "properties" not in finished
    assert "body_duration_ms" not in finished


def test_pytest_and_unittest_skips_are_skips():
    # pytest gives its skip exception the module "builtins": it is told by
    # its name and base, and by isinstance once pytest is loaded.
    class OwnSkip(unittest.SkipTest):
        pass

    class Skipped(Exception):  # a user's class of that name is no skip
        pass

    def pytest_skip():
        pytest.skip("not on this platform")

    def importorskip():
        pytest.importorskip("aye_no_such_module")

    def unittest_skip():
        raise OwnSkip("own reason")

    def users_skipped():
        raise Skipped("mine")

    def pytest_fail():
        pytest.fail("no")

    module = Lifetime("t.py", "module", lambda: pytest.skip("whole module"))
    sink, _ = run(
        [
            Test("t.py::test_pytest_skip", pytest_skip),
            Test("t.py::test_importorskip", importorskip),
            Test("t.py::test_unittest_skip", unittest_skip),
            Test("t.py::test_users_skipped", users_skipped),
            Test("t.py::test_fail", pytest_fail),
            Test("t.py::test_in_skipped_module", lambda: None, lifetimes=[module]),
            Test("t.py::test_setup", lambda: None, setup=pytest_skip),
        ]
    )
    assert outcomes(sink) == {
        "t.py::test_pytest_skip": "skipped",
        "t.py::test_importorskip": "skipped",
        "t.py::test_unittest_skip": "skipped",
        "t.py::test_users_skipped": "error",
        "t.py::test_fail": "error",
        "t.py::test_in_skipped_module": "skipped",
        "t.py::test_setup": "skipped",
    }
    reasons = {e["id"]: e.get("reason") for e in events_of(sink, "test_finished")}
    assert reasons["t.py::test_pytest_skip"] == "not on this platform"
    assert reasons["t.py::test_importorskip"].startswith("could not import 'aye_no_such_module'")
    assert reasons["t.py::test_unittest_skip"] == "own reason"
    assert reasons["t.py::test_in_skipped_module"] == "whole module"
    assert reasons["t.py::test_setup"] == "not on this platform"


def test_pytests_skip_is_told_without_pytest_loaded():
    # The classes as pytest defines them, as an adapter meets them when
    # _pytest.outcomes isn't the module pytest loaded (or isn't loaded).
    class OutcomeException(BaseException):
        def __init__(self, msg=None, pytrace=True):
            super().__init__(msg)
            self.msg = msg
            self.pytrace = pytrace

    class Skipped(OutcomeException):
        __module__ = "builtins"

    class Failed(OutcomeException):
        __module__ = "builtins"

    class Bare(BaseException):
        pass

    Bare.__name__ = Bare.__qualname__ = "Skipped"
    Bare.__module__ = "builtins"
    with_attributes = Bare("x")
    with_attributes.msg, with_attributes.pytrace = "x", True
    assert engine.is_skip(Skipped("why"))
    assert engine.skip_reason(Skipped("why")) == "why"
    assert not engine.is_skip(Failed("no"))
    assert engine.is_skip(with_attributes)
    assert not engine.is_skip(Bare("x"))
    assert not engine.is_skip(ValueError("x"))


def test_a_bad_outcome_carries_its_rerun_with_the_arguments_after_the_double_dash():
    def fail():
        raise AssertionError

    blocked = Lifetime("t.py::A", "class", lambda: 1 / 0)
    tests = [
        Test("t.py::test_pass", lambda: None),
        Test("t.py::test_fail", fail),
        Test("t.py::A::test_blocked", lambda: None, lifetimes=[blocked]),
        Test("t.py::test_xfail", fail, expected="known"),
    ]
    sink, _ = run(tests, policy={"framework_args": ["--db=postgres", "-x y"]})
    finished = {e["id"]: e for e in events_of(sink, "test_finished")}
    rerun = ["--", "--db=postgres", "-x y"]
    assert finished["t.py::test_fail"]["reproduce"] == {
        "command": ["aye", "t.py::test_fail", *rerun]
    }
    assert finished["t.py::A::test_blocked"]["reproduce"] == {
        "command": ["aye", "t.py::A::test_blocked", *rerun]
    }
    assert "reproduce" not in finished["t.py::test_pass"]
    assert "reproduce" not in finished["t.py::test_xfail"]
    sink, _ = run([Test("t.py::test_fail", fail)])
    (finished,) = events_of(sink, "test_finished")
    assert finished["reproduce"] == {"command": ["aye", "t.py::test_fail"]}


def test_tests_that_patch_the_standard_library_change_nothing_reported(tmp_path, monkeypatch):
    # A test may patch what the engine itself uses and leave it patched for
    # the tests after it (a module fixture, a leak): every result is still
    # reported, with its paths, durations and failures.
    source = tmp_path / "test_mod.py"
    source.write_text("def check():\n    assert 1 == 2, 'nope'\n")
    namespace = {}
    exec(compile(source.read_text(), str(source), "exec"), namespace)

    def break_everything():
        broken = mock.MagicMock(side_effect=RuntimeError("patched"))
        for module, name in [
            (os.path, "abspath"),
            (os.path, "relpath"),
            (os.path, "join"),
            (os.path, "isabs"),
            (os.path, "normpath"),
            (os, "getcwd"),
            (traceback, "extract_tb"),
            (traceback, "format_exception"),
            (math, "isfinite"),
            (hashlib, "sha1"),
        ]:
            monkeypatch.setattr(module, name, broken)
        monkeypatch.setattr(time, "perf_counter", lambda: float("nan"))

    module = Lifetime("test_mod.py", "module", break_everything)
    tests = [
        Test("test_mod.py::test_a", lambda: None, lifetimes=[module]),
        Test("test_mod.py::test_b", namespace["check"], lifetimes=[module]),
        Test("test_mod.py::test_c", lambda: None),
    ]
    sink = engine.ListSink(batch=0)
    executor = FakeExecutor(str(tmp_path), tests)
    try:
        executor.run(Batch({"id": 0, "adapter": "fake", "ids": [t.id for t in tests]}), sink)
    finally:
        monkeypatch.undo()
    assert outcomes(sink) == {
        "test_mod.py::test_a": "passed",
        "test_mod.py::test_b": "failed",
        "test_mod.py::test_c": "passed",
    }
    failure = {e["id"]: e for e in events_of(sink, "test_finished")}["test_mod.py::test_b"][
        "failures"
    ][0]
    assert failure["location"] == {"path": "test_mod.py", "line": 2}
    assert failure["message"] == "nope"
    assert "assert 1 == 2" in failure["detail"]
    assert len(failure["signature"]) == 16
    for event in sink.events:
        assert math.isfinite(event["t_ms"])
        if "duration_ms" in event:
            assert math.isfinite(event["duration_ms"])


def test_a_failure_is_built_whatever_the_exception_does():
    class Broken(Exception):
        def __str__(self):
            raise RuntimeError("no text")

    try:
        raise Broken
    except Broken as exc:
        failure = engine.Failure.from_exception(exc, engine.Paths("/repo"), phase=Phase.TEST)
    data = failure.to_dict()
    assert data["exception"] == "Broken"
    assert data["message"] == "Broken"
    paths = engine.Paths("/repo")
    assert paths.relative("/repo/a/b.py") == "a/b.py"
    assert paths.relative("/repo") == "."
    assert paths.relative("/repository/x.py") == "/repository/x.py"
    assert paths.relative("/other/x.py") == "/other/x.py"
    assert paths.relative("<doctest x[0]>") == "<doctest x[0]>"
    assert engine.Paths("/").relative("/a/b.py") == "a/b.py"
    assert engine.Paths("/").relative("/") == "."
    assert paths.relative("/repo/") == "."
    assert paths.relative("/repo/a/../b.py") == "b.py"


def test_os_path_is_kept_as_it_was_whatever_a_test_patches(monkeypatch, tmp_path):
    (tmp_path / "f.py").write_text("")
    file, directory, cwd = str(tmp_path / "f.py"), str(tmp_path), os.getcwd()
    monkeypatch.setattr(os.path, "join", lambda *parts: "/nowhere")
    monkeypatch.setattr(os.path, "isfile", lambda path: False)
    assert engine.os_path.join("a", "b") == os.sep.join(["a", "b"])
    # Functions calling the patched ones are kept whole too.
    assert engine.os_path.abspath("x") == os.sep.join([cwd, "x"])
    assert engine.os_path.relpath(file, directory) == "f.py"
    assert engine.os_path.realpath(file).endswith("f.py")
    assert engine.os_path.isfile(file)


def test_loading_sees_os_path_unpatched_and_the_patch_comes_back(monkeypatch):
    patched = lambda *parts: "/nowhere"
    monkeypatch.setattr(os.path, "join", patched)
    with engine.unpatched_os_path():
        assert os.path.join("a", "b") == os.sep.join(["a", "b"])
    assert os.path.join is patched


def test_captured_streams_have_bytes_an_encoding_and_a_file():
    result = engine.Result("t")
    with engine.captured(result):
        sys.stdout.write("text\n")
        sys.stdout.buffer.write(b"bytes\n")
        assert sys.stdout.encoding == "utf-8"
        os.write(sys.stderr.fileno(), b"through the fd\n")
        print("then closed", file=sys.stderr)
        sys.stderr.close()
    assert result.stdout == "text\nbytes\n"
    # What went through the file comes after what went through the stream.
    assert result.stderr == "then closed\nthrough the fd\n"
    assert not sys.stderr.closed


def test_captured_text_utf8_cant_hold_is_replaced():
    # As pytest's default capture writes it (errors="replace"), and as
    # `python -m unittest -b` takes it: a lone surrogate fails no test.
    result = engine.Result("t")
    with engine.captured(result):
        print("a\udcffb")
        assert sys.stdout.errors == "replace"
        assert sys.stdout.encoding == "utf-8"
        sys.stdout.buffer.write(b"\xff\n")
        # A test may ask for strict encoding, as under pytest.
        sys.stderr.reconfigure(errors="strict")
        with pytest.raises(UnicodeEncodeError):
            sys.stderr.write("\udcff")
    assert result.stdout == "a?b\n\ufffd\n"
    assert result.stderr == ""


def test_a_site_tells_failures_without_a_location_apart():
    one = engine.Failure("2 + 2: expected '5', got '4'", site="m.py::m.t[1]").to_dict()
    two = engine.Failure("1 + 1: expected '3', got '2'", site="m.py::m.t[0]").to_dict()
    assert one["signature"] != two["signature"]
    assert "site" not in one
    same = engine.Failure("2 + 2: expected '5', got '4'", site="m.py::m.t[1]").to_dict()
    assert same["signature"] == one["signature"]
    # With a location, the site changes nothing.
    located = engine.Failure("x", location=("m.py", 3), site="a").to_dict()
    assert located["signature"] == engine.Failure("x", location=("m.py", 3)).to_dict()["signature"]


def test_with_maxfail_a_failed_lifetime_is_reported_before_the_tests_it_blocks():
    def broken():
        raise RuntimeError("database is down")

    cls = Lifetime("m::A", "class", broken)
    tests = [
        Test("m::A::t1", lambda: None, lifetimes=[cls]),
        Test("m::A::t2", lambda: None, lifetimes=[cls]),
    ]
    sink, _ = run(tests, policy={"maxfail": 1})
    kinds = [(e["event"], e.get("id") or e.get("lifetime")) for e in sink.events]
    assert kinds.index(("lifetime_finished", "m::A")) < kinds.index(("test_finished", "m::A::t1"))
    assert len(events_of(sink, "lifetime_finished")) == 1
    sink, _ = run(tests)
    kinds = [(e["event"], e.get("id") or e.get("lifetime")) for e in sink.events]
    assert kinds.index(("lifetime_finished", "m::A")) > kinds.index(("test_finished", "m::A::t2"))


# ------------------------------------------------------------------ comparisons


def test_compare_says_which_keys_of_a_dict_differ():
    found = engine.compare({"apple": 4, "pear": 2}, {"apple": 4, "pear": 3}, "right")
    assert found == {
        "kind": "dict",
        "summary": "dict differs in 1 of 2 keys",
        "same": 1,
        "differs": [{"at": "['pear']", "left": "2", "right": "3"}],
    }
    actual, expected = {"a": 1, "b": 2, "x": 0}, {"a": 9, "b": 2, "c": 3}
    found = engine.compare(actual, expected, "right")
    assert found["summary"] == "dict differs in 1 of 2 keys; 1 key missing, 1 extra"
    assert found["differs"] == [
        {"at": "['a']", "left": "1", "right": "9"},
        {"at": "['x']", "left": "0"},
        {"at": "['c']", "right": "3"},
    ]
    # Missing and extra are said of the actual value, whichever side it is.
    assert engine.compare(expected, actual, "left")["summary"] == (
        "dict differs in 1 of 2 keys; 1 key missing, 1 extra"
    )
    assert engine.compare(actual, expected)["summary"] == (
        "dict differs in 1 of 2 keys; 1 key only in left, 1 only in right"
    )
    assert engine.compare({"a": 1}, {}, "right") == {
        "kind": "dict",
        "summary": "dict: 1 key extra",
        "differs": [{"at": "['a']", "left": "1"}],
    }
    assert engine.compare(collections.OrderedDict(a=1), {"a": 2})["kind"] == "dict"


def test_compare_says_which_positions_of_a_sequence_differ():
    found = engine.compare([1, 2, 3, 4, 5], [1, 9, 3, 8, 5, 6], "right")
    assert found == {
        "kind": "list",
        "summary": "list differs at 2 of 5 positions; 1 item missing",
        "same": 3,
        "differs": [
            {"at": "[1]", "left": "2", "right": "9"},
            {"at": "[3]", "left": "4", "right": "8"},
            {"at": "[5]", "right": "6"},
        ],
    }
    assert engine.compare([1, 2], [1], "right")["summary"] == "list: 1 item extra"
    assert engine.compare((1, 2), (2, 1)) == {
        "kind": "tuple",
        "summary": "tuple differs at 2 of 2 positions; the same items in another order",
        "differs": [
            {"at": "[0]", "left": "1", "right": "2"},
            {"at": "[1]", "left": "2", "right": "1"},
        ],
    }
    # Unhashable items are never said to be reordered.
    assert engine.compare([[1], [2]], [[2], [1]])["summary"] == "list differs at 2 of 2 positions"


def test_compare_lists_at_most_fifty_differences():
    found = engine.compare(list(range(80)), list(range(1, 81)))
    assert len(found["differs"]) == engine.MAX_DIFFERENCES
    assert found["omitted"] == 30
    assert found["differs"][-1] == {"at": "[49]", "left": "49", "right": "50"}


def test_compare_says_what_a_set_misses_and_has_extra():
    assert engine.compare({1, 2, 3}, {2, 3, 4, 5}, "right") == {
        "kind": "set",
        "summary": "set: 2 missing, 1 extra",
        "same": 2,
        "differs": [{"left": "1"}, {"right": "4"}, {"right": "5"}],
    }
    assert engine.compare(frozenset({"a"}), frozenset({"b"}))["summary"] == (
        "set: 1 only in left, 1 only in right"
    )


def test_compare_says_where_strings_first_differ():
    found = engine.compare("hello\nwrld\n", "hello\nworld\n", "right")
    assert found == {
        "kind": "str",
        "summary": "strings differ at line 2, column 2",
        "differs": [{"at": "line 2, column 2", "left": "'wrld'", "right": "'world'"}],
    }
    # The lines' ends, when they are what differs; no line on one side.
    assert engine.compare("abc", "abc\n")["differs"] == [
        {"at": "line 1, column 4", "left": "'abc'", "right": "'abc\\n'"}
    ]
    assert engine.compare("a\n", "a\nb")["differs"] == [{"at": "line 2, column 1", "right": "'b'"}]
    # A long line is shown around where it differs.
    long = "x" * 500
    found = engine.compare(long + "a", long + "b")
    assert found["summary"] == "strings differ at line 1, column 501"
    shown = found["differs"][0]["left"]
    assert len(shown) <= engine.MAX_VALUE
    assert shown.startswith("...'x") and shown.endswith("a'")


def test_compare_says_where_bytes_first_differ():
    assert engine.compare(b"abcdef", b"abcXefg") == {
        "kind": "bytes",
        "summary": "bytes differ at index 3; lengths 6 and 7",
        "differs": [{"at": "[3]", "left": "b'abcdef'", "right": "b'abcXefg'"}],
    }
    found = engine.compare(bytes(1000) + b"a", bytes(1000) + b"b")
    shown = found["differs"][0]["left"]
    assert found["differs"][0]["at"] == "[1000]"
    assert shown.startswith("...b'\\x00") and shown.endswith("a'")


def test_compare_says_which_fields_differ():
    @dataclasses.dataclass
    class Point:
        x: int
        y: int
        label: str = dataclasses.field(default="", compare=False)

    assert engine.compare(Point(1, 2, "a"), Point(1, 3, "b"), "right") == {
        "kind": "dataclass",
        "summary": "Point differs in 1 of 2 fields",
        "same": 1,
        "differs": [{"at": ".y", "left": "2", "right": "3"}],
    }
    Pair = collections.namedtuple("Pair", "a b")
    assert engine.compare(Pair(1, 2), Pair(1, 5)) == {
        "kind": "namedtuple",
        "summary": "Pair differs in 1 of 2 fields",
        "same": 1,
        "differs": [{"at": ".b", "left": "2", "right": "5"}],
    }
    # A namedtuple and a tuple compare as tuples, as == compares them.
    assert engine.compare(Pair(1, 2), (1, 5))["kind"] == "tuple"


def test_compare_shows_dates_and_times_in_iso_format():
    utc = datetime.timezone.utc
    found = engine.compare(
        datetime.datetime(2026, 10, 15, 18, 40, tzinfo=utc),
        datetime.datetime(2026, 10, 11, 9, 30, tzinfo=utc),
        "right",
    )
    assert found == {
        "kind": "datetime",
        "summary": "datetime differs by 4 days, 9:10:00",
        "differs": [{"left": "2026-10-15T18:40:00+00:00", "right": "2026-10-11T09:30:00+00:00"}],
    }
    found = engine.compare(datetime.date(2026, 10, 15), datetime.date(2026, 10, 11))
    assert found["summary"] == "date differs by 4 days"
    assert engine.compare(datetime.time(9, 30), datetime.time(9, 31)) == {
        "kind": "time",
        "summary": "time differs",
        "differs": [{"left": "09:30:00", "right": "09:31:00"}],
    }
    aware = datetime.datetime(2026, 1, 1, tzinfo=utc)
    naive = datetime.datetime(2026, 1, 1)  # noqa: DTZ001 - naive on purpose
    assert engine.compare(aware, naive)["summary"] == "datetime differs"


def test_compare_says_how_far_apart_numbers_are():
    assert engine.compare(9.44, 9.45, "right") == {"kind": "number", "summary": "differ by 0.01"}
    assert engine.compare(3, 5)["summary"] == "differ by 2"
    assert engine.compare(2, 2.5)["summary"] == "differ by 0.5"
    found = engine.compare(decimal.Decimal("9.45"), decimal.Decimal("9.4"))
    assert found["summary"] == "differ by 0.05"
    assert engine.compare(fractions.Fraction(1, 3), 0)["summary"] == "differ by 1/3"
    assert engine.compare(float("nan"), 1.0) is None
    assert engine.compare(True, 2) is None


def test_compare_descends_into_the_one_entry_that_differs():
    found = engine.compare(
        {"db": {"host": "h", "port": [1, {"tls": False}]}, "debug": False},
        {"db": {"host": "h", "port": [1, {"tls": True}]}, "debug": False},
        "right",
    )
    assert found == {
        "kind": "dict",
        "summary": "['db']['port'][1]: dict differs in 1 of 1 key",
        "differs": [{"at": "['db']['port'][1]['tls']", "left": "False", "right": "True"}],
    }
    # Two entries differ: no descent.
    found = engine.compare({"a": [1], "b": [2]}, {"a": [3], "b": [4]})
    assert [d["at"] for d in found["differs"]] == ["['a']", "['b']"]
    # Not into values of two kinds, nor into text.
    assert engine.compare({"a": [1]}, {"a": (1,)})["differs"] == [
        {"at": "['a']", "left": "[1]", "right": "(1,)"}
    ]
    assert engine.compare(["ab"], ["ac"])["differs"] == [
        {"at": "[0]", "left": "'ab'", "right": "'ac'"}
    ]


def test_compare_descends_at_most_five_levels():
    def nested(leaf, depth):
        value = leaf
        for _ in range(depth):
            value = [value]
        return value

    found = engine.compare(nested(1, 8), nested(2, 8))
    assert found["summary"] == "[0][0][0][0]: list differs at 1 of 1 position"
    assert found["differs"] == [{"at": "[0][0][0][0][0]", "left": "[[[1]]]", "right": "[[[2]]]"}]


def test_compare_gives_nothing_it_cant_tell():
    class Broken:
        def __eq__(self, other):
            raise ValueError("no comparing")

        __hash__ = None

    class Unshowable:
        def __repr__(self):
            raise ValueError("no repr")

    assert engine.compare([Broken()], [Broken()]) is None
    assert engine.compare([Unshowable()], [1]) is None
    assert engine.compare({"a": 1}, [1]) is None
    assert engine.compare(1, "1") is None
    assert engine.compare(None, 1) is None
    assert engine.compare(object(), object()) is None


# ------------------------------------------------------------------ frames


def failing(tmp_path, source, entry="check", rewrite=True):
    """The failure calling ``entry`` in ``source`` raises: written under
    ``tmp_path`` (the root), its asserts rewritten unless told not to."""
    path = tmp_path / "test_mod.py"
    path.write_text(source, encoding="utf-8")
    if rewrite:
        code = aye_rewrite.rewrite_source(source, str(path))
    else:
        code = compile(source, str(path), "exec")
    namespace = {"__name__": "test_mod"}
    exec(code, namespace)
    try:
        namespace[entry]()
    except Exception as exc:  # noqa: BLE001 - the failure under test
        failure = engine.Failure.from_exception(exc, engine.Paths(str(tmp_path)), phase=Phase.TEST)
        return failure.to_dict()
    raise AssertionError(f"{entry} passed")


def frame_of(data, function):
    (frame,) = [frame for frame in data["frames"] if frame["function"] == function]
    return frame


CART = """\
class Cart:
    def summary(self):
        return {"apple": 4, "pear": 2}


def check():
    cart = Cart()
    price = 3
    assert cart.summary() == {"apple": 4, "pear": price}
"""


def test_an_asserts_frame_points_at_its_actual_operand(tmp_path):
    frame = frame_of(failing(tmp_path, CART), "check")
    assert frame["source"] == 'assert cart.summary() == {"apple": 4, "pear": price}'
    # The dict isn't a literal (``price``): no expected side, so the whole test.
    assert frame["source"][slice(*frame["cols"])] == (
        'cart.summary() == {"apple": 4, "pear": price}'
    )
    frame = frame_of(failing(tmp_path, CART.replace("price}", "3}")), "check")
    # The literal is expected: the span is the other operand's.
    assert frame["cols"] == [7, 21]
    assert frame["source"][7:21] == "cart.summary()"
    assert frame["before"] == ["cart = Cart()", "price = 3"]


def test_without_an_expected_side_the_span_is_the_whole_test(tmp_path):
    source = "def check():\n    a, b = 1, 2\n    assert a == b, 'why'\n"
    frame = frame_of(failing(tmp_path, source), "check")
    assert frame["source"][slice(*frame["cols"])] == "a == b"
    source = "def check():\n    ok = lambda: False\n    assert ok()\n"
    frame = frame_of(failing(tmp_path, source), "check")
    assert frame["source"][slice(*frame["cols"])] == "ok()"
    # A test that starts on a later line has no span.
    source = "def check():\n    x = 1\n    assert (\n        x == 2\n    )\n"
    assert "cols" not in frame_of(failing(tmp_path, source), "check")


def test_spans_count_characters_not_bytes(tmp_path):
    source = "def check():\n    café = 'é'\n    assert 'ü' + café == 'ü' + 'e'\n"
    frame = frame_of(failing(tmp_path, source), "check")
    assert frame["source"][slice(*frame["cols"])] == "'ü' + café == 'ü' + 'e'"


@pytest.mark.skipif(sys.version_info < (3, 11), reason="co_positions is Python 3.11's")
def test_an_exceptions_frames_point_at_the_instruction_that_raised(tmp_path):
    source = "import json\n\n\ndef check():\n    text = '{'\n    return 1 + len(json.loads(text))\n"
    data = failing(tmp_path, source, rewrite=False)
    frame = frame_of(data, "check")
    assert frame["source"][slice(*frame["cols"])] == "json.loads(text)"
    # Frames outside the project point too.
    assert "cols" in frame_of(data, "decode")
    # A span that covers the whole line is left out.
    source = "import json\n\n\ndef check():\n    json.loads('{')\n"
    frame = frame_of(failing(tmp_path, source, rewrite=False), "check")
    assert frame["source"] == "json.loads('{')" and "cols" not in frame


@pytest.mark.skipif(sys.version_info >= (3, 11), reason="Python 3.10 has no co_positions")
def test_on_python_3_10_only_asserts_have_spans(tmp_path):
    source = "import json\n\n\ndef check():\n    return 1 + len(json.loads('{'))\n"
    data = failing(tmp_path, source, rewrite=False)
    assert all("cols" not in frame for frame in data["frames"])
    frame = frame_of(failing(tmp_path, CART.replace("price}", "3}")), "check")
    assert frame["cols"] == [7, 21]


def test_the_lines_before_keep_their_indentation_relative_to_the_failing_one(tmp_path):
    source = "def check():\n    for i in range(3):\n\n        assert i < 1\n"
    frame = frame_of(failing(tmp_path, source), "check")
    # Relative to the least indented of them; a blank line stays blank.
    assert frame["before"] == ["for i in range(3):", ""]
    assert frame["indent"] == 4
    source = "def check():\n    assert False\n"
    frame = frame_of(failing(tmp_path, source), "check")
    assert frame["before"] == ["def check():"]
    assert frame["indent"] == 4
    source = "def check():\n    x = 1\n    assert x == 2\n"
    frame = frame_of(failing(tmp_path, source), "check")
    assert frame["before"] == ["def check():", "    x = 1"]
    assert frame["indent"] == 4
    source = "def check():\n    if True:\n        x = 1\n    assert x == 2\n"
    frame = frame_of(failing(tmp_path, source), "check")
    assert frame["before"] == ["if True:", "    x = 1"]
    assert "indent" not in frame
    source = "assert False\n"
    path = tmp_path / "top.py"
    path.write_text(source)
    try:
        exec(compile(source, str(path), "exec"), {})
    except AssertionError as exc:
        data = engine.Failure.from_exception(exc, engine.Paths(str(tmp_path))).to_dict()
    assert "before" not in frame_of(data, "<module>")


def test_the_innermost_project_frame_shows_its_locals(tmp_path):
    source = (
        "import os\n\n\n"
        "class Box:\n    pass\n\n\n"
        "def helper():\n    pass\n\n\n"
        "def check():\n"
        "    zeta, alpha = 'z' * 500, [1, 2]\n"
        "    module, function, cls, box = os, helper, Box, Box()\n"
        "    __hidden__ = 1\n"
        "    assert alpha == [1, 3]\n"
    )
    frame = frame_of(failing(tmp_path, source), "check")
    # Sorted; no module, function, class, dunder or rewriter temporary.
    assert [name for name, _ in frame["locals"]] == ["alpha", "box", "zeta"]
    values = dict(frame["locals"])
    assert values["alpha"] == "[1, 2]"
    assert len(values["zeta"]) == engine.MAX_VALUE and "..." in values["zeta"]
    many = "".join(f"    v{i:02} = {i}\n" for i in range(30))
    frame = frame_of(failing(tmp_path, f"def check():\n{many}    assert False\n"), "check")
    assert [name for name, _ in frame["locals"]] == [f"v{i:02}" for i in range(20)]


def test_a_self_with_nothing_to_show_is_left_out(tmp_path):
    source = (
        "class TestA:\n    def test(self):\n        x = 1\n        assert x == 2\n\n\n"
        "class TestB:\n    def __repr__(self):\n        return 'B()'\n\n"
        "    def test(self):\n        assert False\n\n\n"
        "def check_a():\n    TestA().test()\n\n\n"
        "def check_b():\n    TestB().test()\n"
    )
    assert frame_of(failing(tmp_path, source, "check_a"), "test")["locals"] == [["x", "1"]]
    assert frame_of(failing(tmp_path, source, "check_b"), "test")["locals"] == [["self", "B()"]]


def test_only_the_innermost_project_frame_shows_its_surroundings(tmp_path):
    installed = tmp_path / ".venv" / "lib" / "site-packages"
    installed.mkdir(parents=True)
    (installed / "lib.py").write_text(
        "def parse(text):\n    secret = 1\n    raise ValueError(text)\n"
    )
    source = (
        "import importlib.util\n"
        f"spec = importlib.util.spec_from_file_location('lib', {str(installed / 'lib.py')!r})\n"
        "lib = importlib.util.module_from_spec(spec)\n"
        "spec.loader.exec_module(lib)\n\n\n"
        "def outer():\n    inner()\n\n\n"
        "def inner():\n    text = 'bad'\n    lib.parse(text)\n\n\n"
        "def check():\n    outer()\n"
    )
    data = failing(tmp_path, source, rewrite=False)
    assert [f["function"] for f in data["frames"]][-4:] == ["check", "outer", "inner", "parse"]
    shown = [f["function"] for f in data["frames"] if "locals" in f or "before" in f]
    assert shown == ["inner"]
    assert frame_of(data, "inner")["locals"] == [["text", "'bad'"]]


def test_a_projects_files_are_under_the_root_outside_installed_packages(tmp_path):
    paths = engine.Paths(str(tmp_path))
    assert paths.is_project(str(tmp_path / "tests" / "test_a.py"))
    assert not paths.is_project(str(tmp_path / "venv" / "lib" / "site-packages" / "x.py"))
    assert not paths.is_project(str(tmp_path / "debian" / "dist-packages" / "x.py"))
    assert not paths.is_project(str(tmp_path.parent / "elsewhere.py"))
    assert not paths.is_project("<string>")
    # A virtual environment inside the root is Python's, not the project's.
    inside = engine.Paths(sys.prefix + "/..")
    assert not inside.is_project(os.path.join(sys.prefix, "lib", "x.py"))


def test_a_failure_carries_its_resource_and_hint_and_bounds_its_frames():
    data = engine.Failure(
        "boom",
        phase=Phase.SETUP,
        resource="database",
        hint="did you mean 'cart'?",
        frames=[
            {
                "path": "t.py",
                "line": 3,
                "source": "x" * 1200,
                "cols": [990, 1100],
                "before": ["a" * 2000, "b"],
                "locals": [[f"v{i}", "v" * 500] for i in range(30)],
            },
            {"path": "t.py", "line": 4, "source": "short", "cols": [10, 12]},
        ],
    ).to_dict()
    assert data["resource"] == "database"
    assert data["hint"] == "did you mean 'cart'?"
    first, second = data["frames"]
    assert first["cols"] == [990, 1000]
    assert [len(line) for line in first["before"]] == [1000, 1]
    assert len(first["locals"]) == 20
    assert all(len(value) == engine.MAX_VALUE for _, value in first["locals"])
    assert "cols" not in second
    plain = engine.Failure("boom").to_dict()
    assert "resource" not in plain and "hint" not in plain


def test_an_assertions_description_is_carried():
    assertion = {
        "op": "==",
        "left": "1",
        "right": "2",
        "expected": "right",
        "left_source": "total",
        "right_source": "2",
        "compare": {"kind": "number", "summary": "differ by 1"},
    }
    data = engine.Failure("assert 1 == 2", assertion=assertion).to_dict()
    assert data["assertion"] == {**assertion, "diff": []}
    data = engine.Failure("x", assertion={"op": "==", "left": "1", "right": "2"}).to_dict()
    assert data["assertion"] == {"op": "==", "left": "1", "right": "2", "diff": []}


def test_a_result_carries_its_resources():
    result = engine.Result("t.py::t")
    result.add(Outcome.FAILED)
    assert "resources" not in result.to_fields()
    result.resources = [("price_list", "session"), ("cart", "function")]
    assert result.to_fields()["resources"] == [
        {"name": "price_list", "scope": "session"},
        {"name": "cart", "scope": "function"},
    ]
