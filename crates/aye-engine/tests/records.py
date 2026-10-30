"""The records aye's engine sends, on a fixture set, for the strict round trip
in ``records.rs``.

Reads the engine's source (``aye_engine::ENGINE_MODULE``) on stdin, installs
it as the worker does (``sys.modules["aye_engine"]``, its frames reading
``<aye:aye_engine>``), and writes to stdout every record that ``Sink``,
``Result`` and ``Failure`` build here, one JSON object per line, encoded as
the worker encodes them. The set covers every builder and every field they
fill: real failures with their frames, spans, locals and comparisons, and
hand-made ones with every option, through ``EngineExecutor`` and the sink
directly.
"""

import dataclasses
import datetime
import json
import os
import sys
import tempfile
import types
from collections import namedtuple

# UTF-8 both ways, whatever the locale (cp1252 on Windows): `-I` ignores
# PYTHONUTF8 and PYTHONIOENCODING.
source = sys.stdin.buffer.read().decode("utf-8")
engine = types.ModuleType("aye_engine")
sys.modules["aye_engine"] = engine
exec(compile(source, "<aye:aye_engine>", "exec"), engine.__dict__)  # noqa: S102 - aye's own source

encode = json.JSONEncoder(
    ensure_ascii=False, separators=(",", ":"), default=str, allow_nan=False
).encode

USER_CODE = """\
import logging


def check(basket, expected):
    assert basket == expected


def test_compare_dicts():
    basket = {"apple": 1, "pear": 2}
    check(basket, {"apple": 1, "pear": 3})


def test_raises():
    count = 3
    names = ["a", "b"]
    raise KeyError(f"no such item: {count} of {names}")


def setup_class():
    raise RuntimeError("the class's setup broke\\nsecond line")


def teardown_module():
    raise ValueError("teardown broke")


def test_prints():
    print("to stdout é")
    import sys
    print("to stderr", file=sys.stderr)
    logging.getLogger("shop").warning("low stock")
"""


@dataclasses.dataclass
class Point:
    x: int
    y: int


Pair = namedtuple("Pair", "left right")


def comparisons():
    """``compare``'s answers on each kind it knows."""
    pairs = [
        ({"apple": 1, "pear": 2, "plum": 5}, {"apple": 1, "pear": 3, "fig": 4}, "right"),
        ([1, 2, 3, 4], [1, 2, 4], None),
        ((1, "a"), (1, "b"), "left"),
        ({1, 2, 3}, {2, 3, 4}, None),
        ("first line\nsecond line\n", "first line\nsecond lime\n", "right"),
        (b"\x00\x01binary", b"\x00\x02binary", None),
        (Point(1, 2), Point(1, 3), "right"),
        (Pair(1, [2]), Pair(1, [3]), None),
        (
            datetime.datetime(2026, 10, 29, 9, 0, tzinfo=datetime.timezone.utc),
            datetime.datetime(2026, 10, 29, 9, 5, tzinfo=datetime.timezone.utc),
            None,
        ),
        (1.5, 2.25, "right"),
        (list(range(80)), list(range(1, 81)), None),
        ({"deep": {"er": {"est": [1, {"x": 1}]}}}, {"deep": {"er": {"est": [1, {"x": 2}]}}}, None),
    ]
    return [engine.compare(left, right, expected) for left, right, expected in pairs]


def raised(call):
    try:
        call()
    except BaseException as exc:  # noqa: BLE001 - what it raised is the point
        return exc
    raise AssertionError("expected an exception")


class FixtureExecutor(engine.EngineExecutor):
    """An adapter over ``USER_CODE``: its tests and a class lifetime."""

    adapter = "fixture"

    def __init__(self, root, services, module):
        super().__init__(root, services)
        self.module = module

    def load(self, batch):
        broken_class = engine.Lifetime(
            "class:tests/test_user.py::TestBroken", "class", setup=self.module.setup_class
        )
        module = engine.Lifetime(
            "tests/test_user.py", "module", teardown=self.module.teardown_module
        )
        tests = {
            "tests/test_user.py::test_compare_dicts": engine.Test(
                "tests/test_user.py::test_compare_dicts",
                self.module.test_compare_dicts,
                lifetimes=[module],
            ),
            "tests/test_user.py::test_raises": engine.Test(
                "tests/test_user.py::test_raises", self.module.test_raises, lifetimes=[module]
            ),
            "tests/test_user.py::test_prints": engine.Test(
                "tests/test_user.py::test_prints", self.module.test_prints, lifetimes=[module]
            ),
            "tests/test_user.py::test_skips": engine.Test(
                "tests/test_user.py::test_skips", lifetimes=[module], skip="not on Tuesdays"
            ),
            "tests/test_user.py::test_expected": engine.Test(
                "tests/test_user.py::test_expected",
                self.module.test_raises,
                lifetimes=[module],
                expected="known bug",
            ),
            "tests/test_user.py::test_unexpected": engine.Test(
                "tests/test_user.py::test_unexpected", lifetimes=[module], expected="known bug"
            ),
            "tests/test_user.py::TestBroken::test_blocked": engine.Test(
                "tests/test_user.py::TestBroken::test_blocked", lifetimes=[module, broken_class]
            ),
        }
        return [tests[test_id] for test_id in batch.ids]


class Capture:
    """The ``capture`` service: the engine's own ``captured``."""

    def capture(self, result):
        return engine.captured(result)


def executor_records(root, module, lines):
    services = engine.Services(
        {
            engine.Services.CONTEXT: engine.Context(),
            engine.Services.CLEANUP: engine.Cleanup(),
            engine.Services.CAPTURE: Capture(),
        }
    )
    executor = FixtureExecutor(root, services, module)
    batch = engine.Batch(
        {
            "id": 2,
            "adapter": "fixture",
            "ids": [
                "tests/test_user.py::test_compare_dicts",
                "tests/test_user.py::test_raises",
                "tests/test_user.py::test_prints",
                "tests/test_user.py::test_skips",
                "tests/test_user.py::test_expected",
                "tests/test_user.py::test_unexpected",
                "tests/test_user.py::TestBroken::test_blocked",
            ],
            "policy": {"capture": True, "framework_args": ["--reuse-db"]},
            "context": {"rerun_args": ["-c", "pytest.ini"]},
        }
    )
    sink = engine.Sink(lines.append, batch=batch.id)
    executor.run(batch, sink)


def sink_records(root, module, lines):
    paths = engine.Paths(root)
    sink = engine.Sink(lines.append, batch=5)
    exc = raised(module.test_compare_dicts)
    compared = comparisons()
    assertion = {
        "op": "==",
        "left": "{'apple': 1, 'pear': 2}",
        "right": "{'apple': 1, 'pear': 3}",
        "diff": ["Differing items:", "{'pear': 2} != {'pear': 3}", "x" * 600],
        "expected": "right",
        "left_source": "basket",
        "right_source": "expected",
        "compare": compared[0],
    }
    rich = engine.Failure.from_exception(exc, paths, phase=engine.Phase.TEST, assertion=assertion)
    rich.resource = "basket"
    rich.hint = "did you mean basket?"
    sink.test_started("tests/test_user.py::test_full")
    result = engine.Result("tests/test_user.py::test_full")
    result.add(engine.Outcome.FAILED, rich)
    result.add(
        engine.Outcome.ERROR,
        engine.Failure(
            "teardown broke\nand more",
            phase=engine.Phase.TEARDOWN,
            exception="RuntimeError",
            location=("tests/conftest.py", 7),
            frames=[
                {
                    "path": "tests/conftest.py",
                    "line": 7,
                    "function": "db",
                    "source": "raise RuntimeError('teardown broke')",
                    "cols": [4, 500],
                    "before": ["yield conn", "conn.close()", "# done"],
                    "indent": 4,
                    "locals": [["conn", "<Connection>"], ["n" * 300, "v" * 300]],
                }
            ],
            detail="Traceback\n" + "line\n" * 2000,
            site="tests/conftest.py::db",
            resource="db",
            hint="close the connection first",
        ),
    )
    result.add(engine.Outcome.SKIPPED, engine.Failure(""))
    for index, compare in enumerate(compared):
        result.failures.append(
            engine.Failure(
                f"comparison {index}",
                phase=engine.Phase.TEST,
                exception="AssertionError",
                assertion={"op": "==", "left": "l", "right": "r", "compare": compare},
            )
        )
    result.cause = "class:tests/test_user.py::TestBroken"
    result.reason = "r" * 1500
    result.stdout = "out é\n" * 5000
    result.stderr = "err\ud800\n"
    result.log = "WARNING shop: low stock\n"
    result.duration = 0.0123456
    result.body_duration = 0.0004
    result.reproduce = engine.reproduce(
        "tests/test_user.py::test_full", ["--reuse-db"], ["-c", "pytest.ini"]
    )
    result.properties = [("example_key", "1"), ("k" * 1200, "v" * 9000)]
    result.resources = [("tmp_path", "function"), ("db", "session"), ("x" * 1100, "module")]
    sink.test_finished(result)
    sink.test_finished(engine.Result("tests/test_user.py::test_bare"))
    sink.sub_outcome(
        "tests/test_user.py::test_full",
        "i=1" * 400,
        engine.Outcome.FAILED,
        failure=engine.Failure("subtest failed", phase=engine.Phase.TEST),
        duration=0.001,
    )
    sink.sub_outcome(
        "tests/test_user.py::test_full",
        "i=2",
        engine.Outcome.SKIPPED,
        duration=0.0,
        reason="not on Tuesdays",
    )
    sink.sub_outcome("tests/test_user.py::test_full", "i=3", engine.Outcome.EXPECTED_FAILURE)
    sink.lifetime_started("session", "session")
    sink.lifetime_finished("session", None, 1.5)
    sink.lifetime_finished(
        "class:tests/test_user.py::TestBroken",
        engine.Failure.from_exception(raised(module.setup_class), paths, phase=engine.Phase.SETUP),
        0.25,
    )
    sink.diagnostic("error", "batch-stopped", "", "pytest.exit: stop" + "!" * 9000)
    sink.diagnostic("warning", "duplicate-id", "tests/test_user.py", "twice")
    sink.diagnostic("info", "skipped", "tests/test_skip.py", "skipped by pytest")
    sink.run_property("ARCH", "PPC")
    sink.run_property("n" * 1100, "v" * 9000)
    unbatched = engine.Sink(lines.append)
    unbatched.test_started("tests/test_user.py::test_unbatched")


def main():
    # What the fixture's code prints goes to stderr, out of the records' way.
    out, sys.stdout = sys.stdout, sys.stderr
    with tempfile.TemporaryDirectory() as root:
        os.mkdir(os.path.join(root, "tests"))
        path = os.path.join(root, "tests", "test_user.py")
        with open(path, "w", encoding="utf-8") as file:
            file.write(USER_CODE)
        module = types.ModuleType("test_user")
        module.__file__ = path
        exec(compile(USER_CODE, path, "exec"), module.__dict__)  # noqa: S102 - the fixture's code
        lines = []
        executor_records(root, module, lines)
        sink_records(root, module, lines)
    out.reconfigure(encoding="utf-8", newline="\n")
    for line in lines:
        out.write(encode(line) + "\n")


main()
