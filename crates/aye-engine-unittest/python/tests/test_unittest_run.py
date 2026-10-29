"""Batches of unittest suites, run natively."""

import math
import os
import sys

OUTCOMES = """
    import unittest


    class Outcomes(unittest.TestCase):
        def setUp(self):
            if self._testMethodName == "test_setup_error":
                raise RuntimeError("no database")

        def tearDown(self):
            if self._testMethodName == "test_teardown_error":
                raise OSError("could not close")

        def test_pass(self):
            pass

        def test_fail(self):
            self.assertEqual([1, 2], [1, 3])

        def test_error(self):
            {}["key"]

        def test_setup_error(self):
            pass

        def test_teardown_error(self):
            pass

        def test_cleanup_error(self):
            self.addCleanup(lambda: 1 / 0)

        @unittest.skip("not today")
        def test_skip_mark(self):
            pass

        def test_skip_call(self):
            self.skipTest("imperative")

        @unittest.expectedFailure
        def test_xfail(self):
            self.assertTrue(False)

        @unittest.expectedFailure
        def test_xpass(self):
            pass

        def test_plain_assert(self):
            assert 1 == 2, "plain"
"""

NAMES = [
    "test_pass",
    "test_fail",
    "test_error",
    "test_setup_error",
    "test_teardown_error",
    "test_cleanup_error",
    "test_skip_mark",
    "test_skip_call",
    "test_xfail",
    "test_xpass",
    "test_plain_assert",
]


def ids(names, path="test_outcomes.py", cls="Outcomes"):
    return [f"{path}::{cls}::{name}" for name in names]


def test_outcomes_follow_unittests_results(suite):
    suite.write({"test_outcomes.py": OUTCOMES})
    requested = ids(NAMES)
    run = suite.run(requested)

    assert run.status == 1
    assert run.started == requested
    assert list(run.finished) == requested
    assert {i.split("::")[2]: o for i, o in run.outcomes.items()} == {
        "test_pass": "passed",
        "test_fail": "failed",
        "test_error": "error",
        "test_setup_error": "error",
        "test_teardown_error": "error",
        "test_cleanup_error": "error",
        "test_skip_mark": "skipped",
        "test_skip_call": "skipped",
        "test_xfail": "expected_failure",
        "test_xpass": "failed",
        "test_plain_assert": "failed",
    }
    assert run.diagnostics == []
    assert all(e["batch"] == 3 for e in run.events)


def test_failures_are_structured(suite):
    suite.write({"test_outcomes.py": OUTCOMES})
    run = suite.run(ids(NAMES))
    finished = {i.split("::")[2]: e for i, e in run.finished.items()}

    fail = run.failure("test_outcomes.py::Outcomes::test_fail")
    assert fail["phase"] == "test"
    assert fail["exception"] == "AssertionError"
    assert fail["message"] == "Lists differ: [1, 2] != [1, 3]"
    assert fail["assertion"]["op"] == "=="
    assert fail["assertion"]["left"] == "[1, 2]"
    assert fail["assertion"]["right"] == "[1, 3]"
    assert "First differing element 1:" in fail["assertion"]["diff"]
    assert fail["location"] == {"path": "test_outcomes.py", "line": 17}
    assert [f["function"] for f in fail["frames"]] == ["test_fail"]
    assert fail["detail"].startswith("Traceback (most recent call last):")
    assert "unittest/case.py" not in fail["detail"]
    assert len(fail["signature"]) == 16

    error = run.failure("test_outcomes.py::Outcomes::test_error")
    assert error["exception"] == "KeyError"
    assert error["message"] == "KeyError: 'key'"
    assert "assertion" not in error

    assert run.failure("test_outcomes.py::Outcomes::test_setup_error")["phase"] == "setup"
    teardown = finished["test_teardown_error"]["failures"]
    assert [f["phase"] for f in teardown] == ["teardown"]
    cleanup = run.failure("test_outcomes.py::Outcomes::test_cleanup_error")
    assert cleanup["phase"] == "teardown"
    assert cleanup["exception"] == "ZeroDivisionError"

    assert finished["test_skip_mark"]["reason"] == "not today"
    assert finished["test_skip_call"]["reason"] == "imperative"
    assert finished["test_xfail"]["reason"] == "False is not true"
    assert "failures" not in finished["test_xfail"]
    xpass = run.failure("test_outcomes.py::Outcomes::test_xpass")
    assert xpass["message"] == "unexpected success"
    assert xpass["location"] == {"path": "test_outcomes.py", "line": 42}
    assert "assertion" not in run.failure("test_outcomes.py::Outcomes::test_plain_assert")


def test_comparisons_name_their_operands(suite):
    suite.write(
        {
            "test_ops.py": """
                import unittest

                class Ops(unittest.TestCase):
                    def test_in(self):
                        self.assertIn(3, [1, 2])

                    def test_less(self):
                        self.assertLess(5, 2, "too big")

                    def test_none(self):
                        self.assertIsNone(0)

                    def test_strings(self):
                        self.assertEqual("a\\nb\\n", "a\\nc\\n")

                    def test_helper(self):
                        self.check(1, 2)

                    def check(self, a, b):
                        self.assertEqual(a, b)
            """
        }
    )
    names = ["test_in", "test_less", "test_none", "test_strings", "test_helper"]
    run = suite.run(ids(names, "test_ops.py", "Ops"))
    assertions = {
        i.split("::")[2]: e["failures"][0].get("assertion") for i, e in run.finished.items()
    }
    assert assertions["test_in"] == {"op": "in", "left": "3", "right": "[1, 2]", "diff": []}
    assert assertions["test_less"]["op"] == "<"
    assert assertions["test_none"] == {"op": "is", "left": "0", "right": "None", "diff": []}
    assert assertions["test_strings"]["left"] == repr("a\nb\n")
    assert assertions["test_strings"]["diff"] == ["  a", "- b", "+ c"]
    assert assertions["test_helper"]["left"] == "1"
    helper = run.failure("test_ops.py::Ops::test_helper")
    assert [f["function"] for f in helper["frames"]] == ["test_helper", "check"]
    assert run.failure("test_ops.py::Ops::test_less")["message"] == "5 not less than 2 : too big"


def test_an_equality_says_how_its_operands_differ(suite):
    suite.write(
        {
            "test_equal.py": """
                import unittest

                class Equal(unittest.TestCase):
                    def test_dict(self):
                        summary = {"apple": 4, "pear": 2}
                        self.assertEqual(summary, {"apple": 4, "pear": 3})

                    def test_count(self):
                        self.assertCountEqual([1, 2], [2, 3])

                    def test_less(self):
                        self.assertLess(5, 2)
            """
        }
    )
    run = suite.run(ids(["test_dict", "test_count", "test_less"], "test_equal.py", "Equal"))
    assertions = {i.split("::")[2]: e["failures"][0]["assertion"] for i, e in run.finished.items()}
    # Neither side is known as the expected one.
    assert assertions["test_dict"]["compare"] == {
        "kind": "dict",
        "summary": "dict differs in 1 of 2 keys",
        "same": 1,
        "differs": [{"at": "['pear']", "left": "2", "right": "3"}],
    }
    assert "expected" not in assertions["test_dict"]
    # Order doesn't matter to assertCountEqual; only equality is compared.
    assert "compare" not in assertions["test_count"]
    assert "compare" not in assertions["test_less"]
    frame = run.failure("test_equal.py::Equal::test_dict")["frames"][-1]
    assert frame["locals"] == [
        ["self", "<test_equal.Equal testMethod=test_dict>"],
        ["summary", "{'apple': 4, 'pear': 2}"],
    ]
    assert frame["before"] == ["def test_dict(self):", '    summary = {"apple": 4, "pear": 2}']
    assert frame["indent"] == 4


def test_subtests_are_sub_outcomes(suite):
    suite.write(
        {
            "test_sub.py": """
                import unittest

                class Sub(unittest.TestCase):
                    def test_loop(self):
                        for i in range(3):
                            with self.subTest(i=i):
                                self.assertNotEqual(i, 1)

                    def test_errors(self):
                        with self.subTest("boom"):
                            raise ValueError("bad")

                    def test_skips(self):
                        with self.subTest("later", n=2):
                            self.skipTest("not yet")

                    def test_all_pass(self):
                        for i in range(2):
                            with self.subTest(i=i):
                                pass
            """
        }
    )
    names = ["test_loop", "test_errors", "test_skips", "test_all_pass"]
    run = suite.run(ids(names, "test_sub.py", "Sub"))
    assert run.outcomes == {
        "test_sub.py::Sub::test_loop": "failed",
        "test_sub.py::Sub::test_errors": "error",
        "test_sub.py::Sub::test_skips": "passed",
        "test_sub.py::Sub::test_all_pass": "passed",
    }
    assert run.subs("test_sub.py::Sub::test_loop") == [
        ("(i=0)", "passed"),
        ("(i=1)", "failed"),
        ("(i=2)", "passed"),
    ]
    assert run.subs("test_sub.py::Sub::test_errors") == [("[boom]", "error")]
    assert run.subs("test_sub.py::Sub::test_skips") == [("[later] (n=2)", "skipped")]
    loop = run.finished["test_sub.py::Sub::test_loop"]
    assert [f["message"] for f in loop["failures"]] == ["1 == 1"]
    failed = next(e for e in run.of("sub_outcome") if e["outcome"] == "failed")
    assert failed["failure"]["assertion"]["op"] == "!="


def test_fixtures_are_lifetimes(suite):
    suite.write(
        {
            "test_fix.py": """
                import sys
                import unittest

                LOG = []

                def setUpModule():
                    LOG.append("module up")
                    unittest.addModuleCleanup(lambda: print(LOG, file=sys.stderr))
                    unittest.addModuleCleanup(LOG.append, "module cleanup")

                def tearDownModule():
                    LOG.append("module down")

                class A(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        LOG.append("A up")
                        cls.addClassCleanup(LOG.append, "A cleanup")

                    @classmethod
                    def tearDownClass(cls):
                        LOG.append("A down")

                    def test_a1(self):
                        LOG.append("a1")

                    def test_a2(self):
                        LOG.append("a2")

                class B(unittest.TestCase):
                    def test_b(self):
                        LOG.append("b")
            """
        }
    )
    order = ["A::test_a1", "B::test_b", "A::test_a2"]
    run = suite.run([f"test_fix.py::{name}" for name in order], policy={"capture": False})
    assert set(run.outcomes.values()) == {"passed"}
    assert run.lifetimes() == [
        "+test_fix.py",
        "+test_fix.py::A",
        "-test_fix.py::A",
        "+test_fix.py::B",
        "-test_fix.py::B",
        "+test_fix.py::A",
        "-test_fix.py::A",
        "-test_fix.py",
    ]
    # A class that comes back is set up again, as unittest does.
    assert (
        "['module up', 'A up', 'a1', 'A down', 'A cleanup', 'b', 'A up', 'a2', 'A down', "
        "'A cleanup', 'module down', 'module cleanup']" in run.stderr
    )
    kinds = {e["lifetime"]: e["kind"] for e in run.of("lifetime_started")}
    assert kinds == {"test_fix.py": "module", "test_fix.py::A": "class", "test_fix.py::B": "class"}


def test_a_failed_setup_is_reported_once(suite):
    suite.write(
        {
            "test_setup.py": """
                import unittest

                class Broken(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        raise RuntimeError("class setup failed")

                    @classmethod
                    def tearDownClass(cls):
                        raise AssertionError("never runs")

                    def test_one(self):
                        pass

                    def test_two(self):
                        pass

                class Fine(unittest.TestCase):
                    def test_fine(self):
                        pass

                @unittest.skip("whole class")
                class Skipped(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        raise RuntimeError("never runs")

                    def test_s(self):
                        pass

                class SkipsItself(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        raise unittest.SkipTest("no network")

                    def test_n(self):
                        pass
            """,
            "test_module_setup.py": """
                import unittest

                def setUpModule():
                    raise ValueError("module setup")

                class M(unittest.TestCase):
                    def test_m(self):
                        pass

                class N(unittest.TestCase):
                    def test_n(self):
                        pass
            """,
        }
    )
    requested = [
        "test_setup.py::Broken::test_one",
        "test_setup.py::Broken::test_two",
        "test_setup.py::Fine::test_fine",
        "test_setup.py::Skipped::test_s",
        "test_setup.py::SkipsItself::test_n",
        "test_module_setup.py::M::test_m",
        "test_module_setup.py::N::test_n",
    ]
    run = suite.run(requested)
    assert run.outcomes == {
        "test_setup.py::Broken::test_one": "error",
        "test_setup.py::Broken::test_two": "error",
        "test_setup.py::Fine::test_fine": "passed",
        "test_setup.py::Skipped::test_s": "skipped",
        "test_setup.py::SkipsItself::test_n": "skipped",
        "test_module_setup.py::M::test_m": "error",
        "test_module_setup.py::N::test_n": "error",
    }
    finished = run.finished
    assert finished["test_setup.py::Broken::test_one"]["cause"] == "test_setup.py::Broken"
    assert "failures" not in finished["test_setup.py::Broken::test_one"]
    assert finished["test_setup.py::Skipped::test_s"]["reason"] == "whole class"
    assert finished["test_setup.py::SkipsItself::test_n"]["reason"] == "no network"
    assert finished["test_module_setup.py::N::test_n"]["cause"] == "test_module_setup.py"
    failures = {e["lifetime"]: e.get("failure") for e in run.of("lifetime_finished")}
    broken = failures["test_setup.py::Broken"]
    assert broken["phase"] == "setup"
    assert broken["message"] == "RuntimeError: class setup failed"
    assert broken["location"]["path"] == "test_setup.py"
    assert failures["test_setup.py::Skipped"] is None
    assert failures["test_module_setup.py"]["message"] == "ValueError: module setup"
    # The class lifetimes of a failed module never open.
    assert "+test_module_setup.py::M" not in run.lifetimes()


def test_a_failed_teardown_is_the_lifetimes(suite):
    suite.write(
        {
            "test_teardown.py": """
                import unittest

                class T(unittest.TestCase):
                    @classmethod
                    def tearDownClass(cls):
                        raise OSError("disk full")

                    def test_t(self):
                        pass
            """
        }
    )
    run = suite.run(["test_teardown.py::T::test_t"])
    assert run.outcomes == {"test_teardown.py::T::test_t": "passed"}
    failure = [e for e in run.of("lifetime_finished") if e["lifetime"] == "test_teardown.py::T"]
    assert failure[0]["failure"]["phase"] == "teardown"
    assert failure[0]["failure"]["message"] == "OSError: disk full"


def test_output_is_captured_per_test(suite):
    suite.write(
        {
            "test_out.py": """
                import sys
                import unittest

                class Out(unittest.TestCase):
                    def test_prints(self):
                        print("to stdout")
                        print("to stderr", file=sys.stderr)
            """
        }
    )
    captured = suite.run(["test_out.py::Out::test_prints"])
    output = captured.finished["test_out.py::Out::test_prints"]["output"]
    assert output == {"stdout": "to stdout\n", "stderr": "to stderr\n"}
    shown = suite.run(["test_out.py::Out::test_prints"], policy={"capture": False})
    assert "output" not in shown.finished["test_out.py::Out::test_prints"]
    assert "to stdout" in shown.stderr


def test_maxfail_stops_the_batch(suite):
    suite.write({"test_outcomes.py": OUTCOMES})
    run = suite.run(ids(NAMES), policy={"maxfail": 2})
    assert run.started == ids(["test_pass", "test_fail", "test_error"])
    assert run.status == 1


def test_what_cant_load_is_diagnosed(suite):
    suite.write(
        {
            "test_broken.py": "import nonexistent_module_xyz\n",
            "test_skipmod.py": """
                import unittest
                raise unittest.SkipTest("not on this platform")
            """,
            "test_ok.py": """
                import unittest

                class Ok(unittest.TestCase):
                    def test_ok(self):
                        pass
            """,
            "test_bad_load.py": """
                import unittest

                class X(unittest.TestCase):
                    def test_x(self):
                        pass

                def load_tests(loader, tests, pattern):
                    raise RuntimeError("bad load_tests")
            """,
        }
    )
    run = suite.run(
        [
            "test_broken.py::A::test_a",
            "test_broken.py::A::test_b",
            "test_skipmod.py::S::test_s",
            "test_ok.py::Ok::test_ok",
            "test_ok.py::Ok::test_missing",
            "test_bad_load.py::X::test_x",
            "missing.py::A::test_a",
        ]
    )
    assert run.outcomes == {
        "test_skipmod.py::S::test_s": "skipped",
        "test_ok.py::Ok::test_ok": "passed",
    }
    assert run.finished["test_skipmod.py::S::test_s"]["reason"] == "not on this platform"
    diagnostics = [(d["severity"], d["code"], d["path"]) for d in run.diagnostics]
    assert diagnostics == [
        ("error", "collection-error", "test_broken.py"),
        ("error", "not-found", "test_ok.py"),
        ("error", "collection-error", "test_bad_load.py"),
        ("error", "not-found", "missing.py"),
    ]
    messages = {d["path"]: d["message"] for d in run.diagnostics}
    assert messages["test_broken.py"].startswith("Failed to import test module: test_broken")
    assert "No module named 'nonexistent_module_xyz'" in messages["test_broken.py"]
    assert "<aye:" not in messages["test_broken.py"]
    assert "bad load_tests" in messages["test_bad_load.py"]
    assert run.status == 1


def test_aliases_are_tests_of_their_own_and_duplicates_share_the_id(suite):
    suite.write(
        {
            "test_alias.py": """
                import unittest

                class T(unittest.TestCase):
                    def test_t(self):
                        pass

                Alias = T
            """,
            "test_twice.py": """
                import unittest

                class T(unittest.TestCase):
                    def test_t(self):
                        pass

                def load_tests(loader, tests, pattern):
                    tests.addTests(loader.loadTestsFromTestCase(T))
                    return tests
            """,
        }
    )
    run = suite.run(["test_alias.py::Alias::test_t", "test_alias.py::T::test_t"])
    assert run.started == ["test_alias.py::Alias::test_t", "test_alias.py::T::test_t"]
    assert run.diagnostics == []
    run = suite.run(["test_twice.py::T::test_t"])
    assert run.started == ["test_twice.py::T::test_t", "test_twice.py::T::test_t"]
    assert [(d["severity"], d["code"]) for d in run.diagnostics] == [("warning", "duplicate-id")]


def test_a_retried_batch_runs_what_is_left_of_a_duplicate_id(suite):
    # Finding F41: after a worker crash the runner re-queues a batch with
    # `ran_ids`, the tests that already ran. An id unittest runs twice (a
    # load_tests adding a test again) runs only the copies left, by count.
    suite.write(
        {
            "test_dup.py": """
                import unittest

                class Real(unittest.TestCase):
                    def __init__(self, methodName="runTest", value=0):
                        super().__init__(methodName)
                        self.value = value

                    def test_value(self):
                        self.assertEqual(self.value, 2)

                class Other(unittest.TestCase):
                    def test_other(self):
                        pass

                def load_tests(loader, tests, pattern):
                    suite = unittest.TestSuite()
                    suite.addTest(Real("test_value", value=1))
                    suite.addTest(Real("test_value", value=2))
                    suite.addTests(loader.loadTestsFromTestCase(Other))
                    return suite
            """
        }
    )
    ids = ["test_dup.py::Real::test_value", "test_dup.py::Other::test_other"]

    def outcomes(run):
        return [(e["id"], e["outcome"]) for e in run.of("test_finished")]

    run = suite.run(ids)
    assert outcomes(run) == [
        ("test_dup.py::Real::test_value", "failed"),
        ("test_dup.py::Real::test_value", "passed"),
        ("test_dup.py::Other::test_other", "passed"),
    ]
    run = suite.run(ids, context=suite.context(ran_ids=[ids[0]]))
    assert outcomes(run) == [
        ("test_dup.py::Real::test_value", "passed"),
        ("test_dup.py::Other::test_other", "passed"),
    ]
    run = suite.run(ids, context=suite.context(ran_ids=[ids[0], ids[0], ids[1]]))
    assert outcomes(run) == []
    assert run.status == 5


def test_load_tests_decides_what_runs(suite):
    suite.write(
        {
            "test_load.py": """
                import unittest

                class Real(unittest.TestCase):
                    def __init__(self, methodName="runTest", value=0):
                        super().__init__(methodName)
                        self.value = value

                    def test_value(self):
                        self.assertEqual(self.value, 2)

                def check():
                    pass

                def load_tests(loader, tests, pattern):
                    suite = unittest.TestSuite()
                    suite.addTest(Real("test_value", value=2))
                    suite.addTest(unittest.FunctionTestCase(check))
                    return suite
            """
        }
    )
    run = suite.run(["test_load.py::Real::test_value", "test_load.py::check"])
    # The test load_tests made, with its value, not `Real("test_value")`.
    assert run.outcomes == {
        "test_load.py::Real::test_value": "passed",
        "test_load.py::check": "passed",
    }
    # unittest's own classes get no lifetimes.
    assert run.lifetimes() == [
        "+test_load.py",
        "+test_load.py::Real",
        "-test_load.py::Real",
        "-test_load.py",
    ]


def test_a_package_that_defines_load_tests_runs_its_tests(suite):
    suite.write(
        {
            "pkg/__init__.py": """
                import os
                import unittest

                def load_tests(loader, standard_tests, pattern):
                    found = loader.discover(start_dir=os.path.dirname(__file__), pattern=pattern)
                    keep = unittest.TestSuite()
                    for case in found:
                        keep.addTests(case)
                    standard_tests.addTests(keep)
                    return standard_tests
            """,
            "pkg/test_a.py": """
                import unittest

                class A(unittest.TestCase):
                    def test_a(self):
                        pass
            """,
        }
    )
    run = suite.run(["pkg/test_a.py::A::test_a"])
    assert run.outcomes == {"pkg/test_a.py::A::test_a": "passed"}


def test_async_tests_run(suite):
    suite.write(
        {
            "test_async.py": """
                import asyncio
                import unittest

                class Async(unittest.IsolatedAsyncioTestCase):
                    async def test_sleep(self):
                        await asyncio.sleep(0)

                    async def test_fails(self):
                        await asyncio.sleep(0)
                        self.assertEqual(1, 2)
            """
        }
    )
    run = suite.run(["test_async.py::Async::test_sleep", "test_async.py::Async::test_fails"])
    assert run.outcomes == {
        "test_async.py::Async::test_sleep": "passed",
        "test_async.py::Async::test_fails": "failed",
    }
    failure = run.failure("test_async.py::Async::test_fails")
    assert [f["function"] for f in failure["frames"]] == ["test_fails"]
    assert failure["assertion"]["op"] == "=="


def test_a_keyboard_interrupt_stops_the_batch(suite):
    suite.write(
        {
            "test_stop.py": """
                import unittest

                class Stop(unittest.TestCase):
                    def test_1(self):
                        pass

                    def test_2(self):
                        raise KeyboardInterrupt

                    def test_3(self):
                        pass
            """
        }
    )
    run = suite.run(ids(["test_1", "test_2", "test_3"], "test_stop.py", "Stop"))
    assert run.status == 2
    assert run.outcomes == {
        "test_stop.py::Stop::test_1": "passed",
        "test_stop.py::Stop::test_2": "error",
    }
    stopped = [d for d in run.diagnostics if d["code"] == "batch-stopped"]
    assert stopped[0]["severity"] == "error"
    assert "test_stop.py::Stop::test_3" in stopped[0]["message"]
    # Lifetimes still close.
    assert run.lifetimes()[-1] == "-test_stop.py"


def test_the_context_service_names_the_running_test(suite):
    suite.write(
        {
            "test_ctx.py": """
                import unittest

                class C(unittest.TestCase):
                    def test_c(self):
                        pass
            """
        }
    )
    run = suite.run(["test_ctx.py::C::test_c"], watch_context=True)
    assert run.seen == [["test_ctx.py::C::test_c", ["test_ctx.py", "test_ctx.py::C"]]]


def test_configured_discovery_imports_from_the_top_level_directory(suite):
    suite.write(
        {
            "tests/__init__.py": "",
            "tests/helpers.py": "VALUE = 7\n",
            "tests/cart_check.py": """
                import unittest
                from tests.helpers import VALUE

                class Cart(unittest.TestCase):
                    def test_value(self):
                        self.assertEqual(VALUE, 7)
            """,
        }
    )
    context = suite.context(
        start_directory=str(suite.root / "tests"),
        top_level_directory=str(suite.root),
        pattern="*_check.py",
    )
    run = suite.run(["tests/cart_check.py::Cart::test_value"], context=context)
    assert run.outcomes == {"tests/cart_check.py::Cart::test_value": "passed"}
    assert run.lifetimes()[0] == "+tests/cart_check.py"


def test_status_says_what_happened(suite):
    suite.write(
        {
            "test_ok.py": """
                import unittest

                class Ok(unittest.TestCase):
                    def test_ok(self):
                        pass
            """
        }
    )
    assert suite.run(["test_ok.py::Ok::test_ok"]).status == 0
    assert suite.run([]).status == 5


def test_outcomes_match_unittest_itself(suite):
    """The outcome set against ``python -m unittest``'s own counts."""
    suite.write({"test_outcomes.py": OUTCOMES})
    run = suite.run(ids(NAMES))
    done = suite.unittest("test_outcomes")
    summary = done.stderr.strip().splitlines()[-1]
    assert summary == (
        "FAILED (failures=2, errors=4, skipped=2, expected failures=1, unexpected successes=1)"
    )
    counts = {}
    for outcome in run.outcomes.values():
        counts[outcome] = counts.get(outcome, 0) + 1
    # unittest counts the test whose teardown failed as an error, and the
    # unexpected success apart; aye folds it into failed.
    assert counts == {"passed": 1, "failed": 3, "error": 4, "skipped": 2, "expected_failure": 1}


def test_a_method_the_loader_skips_runs_when_named(suite):
    """As ``python -m unittest test_direct.Direct.check_me`` does."""
    suite.write(
        {
            "test_direct.py": """
                import unittest

                class Direct(unittest.TestCase):
                    def check_me(self):
                        pass

                    def test_listed(self):
                        pass
            """
        }
    )
    run = suite.run(["test_direct.py::Direct::check_me", "test_direct.py::Direct::nothing"])
    assert run.outcomes == {"test_direct.py::Direct::check_me": "passed"}
    assert [(d["code"], d["message"]) for d in run.diagnostics] == [
        ("not-found", "test_direct.py::Direct::nothing: unittest found no such test")
    ]


def test_a_bad_outcome_carries_its_rerun(suite):
    suite.write(
        {
            "test_rerun.py": """
                import unittest

                class Rerun(unittest.TestCase):
                    def test_pass(self):
                        pass

                    def test_fail(self):
                        self.assertEqual(1, 2)

                    @unittest.expectedFailure
                    def test_xfail(self):
                        self.assertEqual(1, 2)

                class Broken(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        raise RuntimeError("down")

                    def test_blocked(self):
                        pass
            """
        }
    )
    names = ["Rerun::test_pass", "Rerun::test_fail", "Rerun::test_xfail", "Broken::test_blocked"]
    requested = [f"test_rerun.py::{name}" for name in names]
    run = suite.run(requested, policy={"framework_args": ["--db", "a b"]})
    reruns = {i: e.get("reproduce") for i, e in run.finished.items()}
    assert reruns == {
        "test_rerun.py::Rerun::test_pass": None,
        "test_rerun.py::Rerun::test_fail": {
            "command": ["aye", "test_rerun.py::Rerun::test_fail", "--", "--db", "a b"]
        },
        "test_rerun.py::Rerun::test_xfail": None,
        "test_rerun.py::Broken::test_blocked": {
            "command": ["aye", "test_rerun.py::Broken::test_blocked", "--", "--db", "a b"]
        },
    }
    plain = suite.run(["test_rerun.py::Rerun::test_fail"])
    assert plain.finished["test_rerun.py::Rerun::test_fail"]["reproduce"] == {
        "command": ["aye", "test_rerun.py::Rerun::test_fail"]
    }
    # C26: aye's own options that shaped the run come before the id.
    configured = suite.run(
        ["test_rerun.py::Rerun::test_fail"],
        context=suite.context(rerun_args=["-c", "alt.ini"]),
    )
    assert configured.finished["test_rerun.py::Rerun::test_fail"]["reproduce"] == {
        "command": ["aye", "-c", "alt.ini", "test_rerun.py::Rerun::test_fail"]
    }


def test_a_lifetimes_traceback_starts_at_the_users_code(suite):
    # As unittest prints it: no frame of aye's (the render report's
    # ``<aye:aye_engine_unittest>`` in ``open``), also for a second error.
    suite.write(
        {
            "test_life.py": """
                import unittest

                class Broken(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        raise RuntimeError("down")

                    def test_blocked(self):
                        pass

                class Late(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        cls.addClassCleanup(cls.clean)

                    @classmethod
                    def tearDownClass(cls):
                        raise OSError("busy")

                    @classmethod
                    def clean(cls):
                        raise ValueError("also")

                    def test_ok(self):
                        pass
            """
        }
    )
    run = suite.run(["test_life.py::Broken::test_blocked", "test_life.py::Late::test_ok"])
    details = {
        e["lifetime"]: e["failure"]["detail"] for e in run.of("lifetime_finished") if "failure" in e
    }
    assert len(details) == 2, details
    for detail in details.values():
        assert "<aye:" not in detail, detail
        assert detail.startswith('Traceback (most recent call last):\n  File "'), detail
    broken, late = sorted(details.items())
    assert broken[1].endswith('raise RuntimeError("down")\nRuntimeError: down\n'), broken
    assert "OSError: busy" in late[1]
    assert late[1].endswith("ValueError: also\n"), late


def test_a_skipped_subtest_carries_its_reason(suite):
    suite.write(
        {
            "test_sub.py": """
                import unittest

                class Sub(unittest.TestCase):
                    def test_skips(self):
                        for i in range(3):
                            with self.subTest(i=i):
                                if i == 1:
                                    self.skipTest("one is skipped")
            """
        }
    )
    run = suite.run(["test_sub.py::Sub::test_skips"])
    # In unittest's order, which before Python 3.11 reports a test's passing
    # subtests after the rest.
    subs = [(e["name"], e["outcome"], e.get("reason")) for e in run.of("sub_outcome")]
    assert sorted(subs) == [
        ("(i=0)", "passed", None),
        ("(i=1)", "skipped", "one is skipped"),
        ("(i=2)", "passed", None),
    ]
    assert run.outcomes == {"test_sub.py::Sub::test_skips": "passed"}


PATCHING = """
    import io
    import math
    import os
    import sys
    import time
    import traceback
    import unittest
    from unittest import mock


    def setUpModule():
        # Patched and never restored, as a leaking fixture leaves them.
        broken = mock.MagicMock(side_effect=RuntimeError("patched"))
        for target in (
            "os.path.abspath",
            "os.path.relpath",
            "os.path.join",
            "os.path.isabs",
            "os.path.isfile",
            "os.path.normpath",
            "os.getcwd",
            "traceback.extract_tb",
            "traceback.format_exception",
            "traceback.TracebackException",
            "math.isfinite",
            "hashlib.sha1",
        ):
            mock.patch(target, broken).start()
        time.perf_counter = lambda: float("nan")
        io.StringIO = broken


    class Patched(unittest.TestCase):
        def test_pass(self):
            print("captured")

        def test_fail(self):
            self.assertEqual([1, 2], [1, 3])

        def test_stdout_left_broken(self):
            sys.stdout = None
"""


def test_tests_that_patch_the_standard_library_change_nothing_reported(suite):
    suite.write(
        {
            "test_patching.py": PATCHING,
            "test_later.py": """
                import unittest

                class Later(unittest.TestCase):
                    def test_after(self):
                        print("still captured")

                    def test_after_fail(self):
                        assert 1 == 2, "plain"
            """,
        }
    )
    requested = [
        "test_patching.py::Patched::test_pass",
        "test_patching.py::Patched::test_fail",
        "test_patching.py::Patched::test_stdout_left_broken",
        "test_later.py::Later::test_after",
        "test_later.py::Later::test_after_fail",
    ]
    run = suite.run(requested)
    assert run.outcomes == {
        "test_patching.py::Patched::test_pass": "passed",
        "test_patching.py::Patched::test_fail": "failed",
        "test_patching.py::Patched::test_stdout_left_broken": "passed",
        "test_later.py::Later::test_after": "passed",
        "test_later.py::Later::test_after_fail": "failed",
    }
    assert run.finished["test_patching.py::Patched::test_pass"]["output"] == {
        "stdout": "captured\n"
    }
    assert run.finished["test_later.py::Later::test_after"]["output"] == {
        "stdout": "still captured\n"
    }
    failure = run.failure("test_patching.py::Patched::test_fail")
    assert failure["location"] == {"path": "test_patching.py", "line": 38}
    assert failure["assertion"]["left"] == "[1, 2]"
    assert run.failure("test_later.py::Later::test_after_fail")["message"] == "plain"
    for event in run.events:
        assert math.isfinite(event["t_ms"])
        if "duration_ms" in event:
            assert math.isfinite(event["duration_ms"]) and event["duration_ms"] >= 0
    assert [e["event"] for e in run.of("lifetime_finished") if e.get("failure")] == []


def test_a_failed_doctest_suite_case_points_at_the_users_code(suite):
    # DocTestSuite's cases run inside doctest.py: its frames are hidden,
    # as unittest's own are.
    suite.write(
        {
            "test_docs.py": '''
                import doctest


                def double(x):
                    """
                    >>> double(3)
                    7
                    """
                    return 2 * x


                def load_tests(loader, tests, pattern):
                    tests.addTests(doctest.DocTestSuite())
                    return tests
            '''
        }
    )
    test_id = "test_docs.py::test_docs.double"
    run = suite.run([test_id])
    assert run.outcomes == {test_id: "failed"}
    failure = run.failure(test_id)
    assert all("doctest.py" not in frame["path"] for frame in failure.get("frames", []))
    assert "location" not in failure or "doctest.py" not in failure["location"]["path"]
    assert "Failed example:" in failure["detail"]


TEARDOWNS = """
    import unittest

    def tearDownModule():
        raise RuntimeError("tearDownModule boom")

    class Resource(unittest.TestCase):
        @classmethod
        def setUpClass(cls):
            cls.addClassCleanup(cls._release)

        @classmethod
        def _release(cls):
            raise RuntimeError("class cleanup boom")

        @classmethod
        def tearDownClass(cls):
            raise RuntimeError("tearDownClass boom")

        def test_ok(self):
            pass
"""


def test_a_failed_teardown_fails_the_batch_as_it_fails_unittest(suite):
    suite.write({"test_teardown.py": TEARDOWNS})
    run = suite.run(["test_teardown.py::Resource::test_ok"])
    assert run.outcomes == {"test_teardown.py::Resource::test_ok": "passed"}
    assert run.lifetimes() == [
        "+test_teardown.py",
        "+test_teardown.py::Resource",
        "-test_teardown.py::Resource!",
        "-test_teardown.py!",
    ]
    # unittest's wasSuccessful() is false: its status is 1.
    assert run.status == 1
    assert suite.unittest("discover").returncode == 1
    (cls,) = [e for e in run.of("lifetime_finished") if e["lifetime"].endswith("Resource")]
    assert cls["failure"]["message"] == "RuntimeError: tearDownClass boom"
    assert "class cleanup boom" in cls["failure"]["detail"]


def test_each_failing_teardown_alone_fails_the_batch(suite):
    sources = {
        "test_class.py": """
            import unittest

            class C(unittest.TestCase):
                @classmethod
                def tearDownClass(cls):
                    raise RuntimeError("tdc")

                def test_ok(self):
                    pass
        """,
        "test_module.py": """
            import unittest

            def tearDownModule():
                raise RuntimeError("tdm")

            class M(unittest.TestCase):
                def test_ok(self):
                    pass
        """,
        "test_cleanup.py": """
            import unittest

            def setUpModule():
                unittest.addModuleCleanup(lambda: 1 / 0)

            class K(unittest.TestCase):
                def test_ok(self):
                    pass
        """,
        "test_context.py": """
            import contextlib
            import unittest

            @contextlib.contextmanager
            def broken():
                yield
                raise OSError("exit boom")

            class E(unittest.TestCase):
                @classmethod
                def setUpClass(cls):
                    cls.enterClassContext(broken())

                def test_ok(self):
                    pass
        """,
    }
    suite.write(sources)
    cases = [("test_class.py", "C"), ("test_module.py", "M"), ("test_cleanup.py", "K")]
    if sys.version_info >= (3, 11):  # enterClassContext
        cases.append(("test_context.py", "E"))
    for path, name in cases:
        test_id = f"{path}::{name}::test_ok"
        run = suite.run([test_id])
        assert run.outcomes == {test_id: "passed"}, path
        assert run.status == 1, path


def test_a_skip_in_a_teardown_is_no_failure(suite):
    suite.write(
        {
            "test_skip_td.py": """
                import unittest

                class S(unittest.TestCase):
                    @classmethod
                    def tearDownClass(cls):
                        raise unittest.SkipTest("nothing to tear down")

                    def test_ok(self):
                        pass
            """
        }
    )
    run = suite.run(["test_skip_td.py::S::test_ok"])
    assert run.status == 0
    assert [e for e in run.of("lifetime_finished") if e.get("failure")] == []
    assert suite.unittest("discover").returncode == 0


def test_a_keyboard_interrupt_in_a_teardown_stops_the_batch(suite):
    sources = {
        "test_class.py": """
            import unittest

            class A(unittest.TestCase):
                @classmethod
                def tearDownClass(cls):
                    raise KeyboardInterrupt

                def test_1(self):
                    pass
        """,
        "test_module.py": """
            import unittest

            def setUpModule():
                def stop():
                    raise KeyboardInterrupt
                unittest.addModuleCleanup(stop)

            class A(unittest.TestCase):
                def test_1(self):
                    pass
        """,
        "test_cleanup.py": """
            import unittest

            class A(unittest.TestCase):
                @classmethod
                def setUpClass(cls):
                    def stop():
                        raise KeyboardInterrupt
                    cls.addClassCleanup(stop)

                def test_1(self):
                    pass
        """,
        "test_b.py": """
            import unittest

            class B(unittest.TestCase):
                def test_b(self):
                    pass
        """,
    }
    suite.write(sources)
    for path in ["test_class.py", "test_module.py", "test_cleanup.py"]:
        run = suite.run([f"{path}::A::test_1", "test_b.py::B::test_b"])
        assert run.status == 2, path
        assert run.outcomes == {f"{path}::A::test_1": "passed"}, path
        (stopped,) = [d for d in run.diagnostics if d["code"] == "batch-stopped"]
        assert "test_b.py::B::test_b" in stopped["message"], path
        failed = [e for e in run.of("lifetime_finished") if e.get("failure")]
        assert failed[0]["failure"]["exception"] == "KeyboardInterrupt", path
        # Every lifetime opened still finishes.
        assert len(run.of("lifetime_finished")) == len(run.of("lifetime_started")), path


def test_an_expected_failure_whose_exception_cant_say_what_it_is(suite):
    suite.write(
        {
            "test_str.py": """
                import unittest

                class BadStr(Exception):
                    def __str__(self):
                        return self.detail

                class S(unittest.TestCase):
                    @unittest.expectedFailure
                    def test_known_bug(self):
                        raise BadStr()
            """
        }
    )
    run = suite.run(["test_str.py::S::test_known_bug"])
    assert run.outcomes == {"test_str.py::S::test_known_bug": "expected_failure"}
    assert run.finished["test_str.py::S::test_known_bug"]["reason"] == "BadStr"
    assert run.status == 0


def test_count_equal_names_the_arguments_it_was_given(suite):
    suite.write(
        {
            "test_count.py": """
                import unittest

                class Count(unittest.TestCase):
                    def test_count_equal(self):
                        self.assertCountEqual(["a", "b", "b"], ["a", "b", "c"])

                    def test_from_generators(self):
                        self.assertCountEqual((x for x in "ab"), iter("abc"))
            """
        }
    )
    run = suite.run(
        ["test_count.py::Count::test_count_equal", "test_count.py::Count::test_from_generators"]
    )
    assertion = run.failure("test_count.py::Count::test_count_equal")["assertion"]
    assert (assertion["left"], assertion["right"]) == ("['a', 'b', 'b']", "['a', 'b', 'c']")
    assertion = run.failure("test_count.py::Count::test_from_generators")["assertion"]
    assert (assertion["left"], assertion["right"]) == ("['a', 'b']", "['a', 'b', 'c']")


def test_every_failed_load_of_a_module_is_reported(suite):
    suite.write(
        {
            "test_lt.py": """
                import unittest

                class A(unittest.TestCase):
                    def test_a(self):
                        pass

                def load_tests(loader, tests, pattern):
                    tests.addTests(loader.loadTestsFromName("missing_plugin_tests"))
                    tests.addTests(loader.loadTestsFromName("test_lt.A.test_renamed"))
                    return tests
            """
        }
    )
    # The module defines load_tests: refinement loads it, and reports it.
    refined = suite.refine(["test_lt.py"])
    assert [c["id"] for c in refined["cases"]] == ["test_lt.py::A::test_a"]
    (error,) = refined["diagnostics"]
    assert error["code"] == "collection-error"
    assert "Failed to import test module: missing_plugin_tests" in error["message"]
    assert "Failed to access attribute:" in error["message"]
    assert "has no attribute 'test_renamed'" in error["message"]
    assert "load_tests" not in error["message"]


def test_captured_streams_behave_as_real_ones(suite):
    suite.write(
        {
            "test_cli.py": """
                import os
                import subprocess
                import sys
                import unittest

                class Output(unittest.TestCase):
                    def test_binary_output(self):
                        sys.stdout.buffer.write(b"\\x00\\x01payload\\n")
                        sys.stdout.flush()

                    def test_encoding(self):
                        self.assertTrue(sys.stdout.encoding.lower().startswith("utf"))

                    def test_fileno(self):
                        self.assertIsInstance(sys.stderr.fileno(), int)
                        os.write(sys.stderr.fileno(), b"through the fd\\n")

                    def test_subprocess(self):
                        subprocess.run(
                            [sys.executable, "-c", "print('from a child')"],
                            stdout=sys.stdout,
                            check=True,
                        )

                    def test_isatty(self):
                        self.assertFalse(sys.stdout.isatty())

                    def test_lone_surrogate(self):
                        # As pytest's default capture (errors="replace") and
                        # `python -m unittest -b` take it: no error.
                        print("a\\udcffb")
            """
        }
    )
    names = [
        "test_binary_output",
        "test_encoding",
        "test_fileno",
        "test_subprocess",
        "test_isatty",
        "test_lone_surrogate",
    ]
    requested = ids(names, "test_cli.py", "Output")
    run = suite.run(requested)
    assert set(run.outcomes.values()) == {"passed"}, run.finished
    assert run.status == 0
    out = run.finished
    assert out["test_cli.py::Output::test_binary_output"]["output"] == {
        "stdout": "\x00\x01payload\n"
    }
    assert out["test_cli.py::Output::test_fileno"]["output"] == {"stderr": "through the fd\n"}
    # A child process writes text with the platform's line ending (\r\n on
    # Windows); the capture keeps its bytes, as pytest's capfd does.
    assert out["test_cli.py::Output::test_subprocess"]["output"] == {
        "stdout": "from a child" + os.linesep
    }
    assert out["test_cli.py::Output::test_lone_surrogate"]["output"] == {"stdout": "a?b\n"}


def test_a_test_that_closes_a_captured_stream_loses_nothing(suite):
    suite.write(
        {
            "test_close.py": """
                import sys
                import unittest

                class A(unittest.TestCase):
                    def test_1(self):
                        pass

                    def test_2_close_stdout(self):
                        print("hello")
                        sys.stdout.close()

                    def test_3_close_stderr_buffer(self):
                        print("err", file=sys.stderr)
                        sys.stderr.buffer.close()

                    def test_4_fail(self):
                        self.fail("x")
            """,
            "test_z.py": """
                import unittest

                class Z(unittest.TestCase):
                    def test_z(self):
                        print("after")
            """,
        }
    )
    requested = [
        *ids(
            ["test_1", "test_2_close_stdout", "test_3_close_stderr_buffer", "test_4_fail"],
            "test_close.py",
            "A",
        ),
        "test_z.py::Z::test_z",
    ]
    run = suite.run(requested)
    assert run.outcomes == {
        "test_close.py::A::test_1": "passed",
        "test_close.py::A::test_2_close_stdout": "passed",
        "test_close.py::A::test_3_close_stderr_buffer": "passed",
        "test_close.py::A::test_4_fail": "failed",
        "test_z.py::Z::test_z": "passed",
    }
    assert run.status == 1
    assert run.finished["test_close.py::A::test_2_close_stdout"]["output"] == {"stdout": "hello\n"}
    assert run.finished["test_z.py::Z::test_z"]["output"] == {"stdout": "after\n"}


def test_a_failed_fixtures_output_is_kept_with_its_failure(suite):
    suite.write(
        {
            "test_db.py": """
                import sys
                import unittest

                class Database(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        print("connecting to postgres at localhost:5433")
                        sys.stderr.write("psql: server closed the connection\\n")
                        raise ConnectionError("could not connect")

                    def test_query(self):
                        pass

                class Teardown(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        print("set up quietly")

                    @classmethod
                    def tearDownClass(cls):
                        print("dropping tables")
                        raise OSError("disk full")

                    def test_t(self):
                        pass
            """
        }
    )
    run = suite.run(["test_db.py::Database::test_query", "test_db.py::Teardown::test_t"])
    failed = {e["lifetime"]: e["failure"] for e in run.of("lifetime_finished") if e.get("failure")}
    setup = failed["test_db.py::Database"]["detail"]
    assert setup.index("ConnectionError") < setup.index("\nStdout:\nconnecting to postgres")
    assert "\nStderr:\npsql: server closed the connection\n" in setup
    teardown = failed["test_db.py::Teardown"]["detail"]
    assert "\nStdout:\ndropping tables\n" in teardown
    assert "set up quietly" not in teardown
    # Without capture, it goes where the worker's stderr goes.
    shown = suite.run(["test_db.py::Database::test_query"], policy={"capture": False})
    (failure,) = [e["failure"] for e in shown.of("lifetime_finished") if e.get("failure")]
    assert "Stdout:" not in failure["detail"]
    assert "connecting to postgres" in shown.stderr


def test_with_maxfail_a_failed_setup_is_reported_before_the_tests_it_blocks(suite):
    suite.write(
        {
            "test_a.py": """
                import unittest

                class A(unittest.TestCase):
                    @classmethod
                    def setUpClass(cls):
                        raise RuntimeError("database unreachable")

                    def test_1(self):
                        pass

                    def test_2(self):
                        pass
            """
        }
    )
    requested = ids(["test_1", "test_2"], "test_a.py", "A")
    run = suite.run(requested, policy={"maxfail": 1})
    kinds = [(e["event"], e.get("id") or e.get("lifetime")) for e in run.events]
    finished_lifetime = kinds.index(("lifetime_finished", "test_a.py::A"))
    assert finished_lifetime < kinds.index(("test_finished", "test_a.py::A::test_1"))
    (failure,) = [e["failure"] for e in run.of("lifetime_finished") if e.get("failure")]
    assert failure["message"] == "RuntimeError: database unreachable"
    assert run.finished["test_a.py::A::test_1"]["cause"] == "test_a.py::A"
    # Reported once.
    assert len([k for k in kinds if k == ("lifetime_finished", "test_a.py::A")]) == 1
    # Without maxfail, after the tests it blocked, as before.
    run = suite.run(requested)
    kinds = [(e["event"], e.get("id") or e.get("lifetime")) for e in run.events]
    assert kinds.index(("lifetime_finished", "test_a.py::A")) > kinds.index(
        ("test_finished", "test_a.py::A::test_2")
    )


def test_a_test_leaving_os_path_patched_breaks_no_later_batch(suite):
    suite.write(
        {
            "tests/__init__.py": "",
            "tests/test_z.py": """
                import os.path
                import unittest

                class Z(unittest.TestCase):
                    def test_1_patches_join(self):
                        os.path.join = lambda *parts: "/nowhere"

                    def test_2(self):
                        pass
            """,
            "tests/test_b.py": """
                import os.path
                import unittest

                # Imported before any test runs, under unittest.
                HERE = os.path.join(os.path.dirname(__file__), "data")

                class T(unittest.TestCase):
                    def test_fails(self):
                        self.fail("expected failure")

                    def test_here(self):
                        self.assertTrue(HERE.endswith("data"), HERE)
            """,
            "tests/pkg/__init__.py": """
                def load_tests(loader, tests, pattern):
                    return loader.discover(__path__[0], pattern, loader._top_level_dir)
            """,
            "tests/pkg/test_p.py": """
                import unittest

                class P(unittest.TestCase):
                    def test_p(self):
                        pass
            """,
        }
    )
    first = suite.batch(["tests/test_z.py::Z::test_1_patches_join", "tests/test_z.py::Z::test_2"])
    second = suite.batch(["tests/test_b.py::T::test_fails", "tests/test_b.py::T::test_here"], id=4)
    third = suite.batch(["tests/pkg/test_p.py::P::test_p"], id=5)
    runs = suite.run_batches([first, second, third])
    assert runs[1].outcomes == {
        "tests/test_b.py::T::test_fails": "failed",
        "tests/test_b.py::T::test_here": "passed",
    }
    assert runs[1].diagnostics == []
    assert runs[2].outcomes == {"tests/pkg/test_p.py::P::test_p": "passed"}
    assert runs[2].diagnostics == []


def test_a_named_file_below_a_load_tests_package_runs_alone(suite):
    suite.write(
        {
            "tests/__init__.py": "",
            "tests/pkg/__init__.py": """
                def load_tests(loader, tests, pattern):
                    tests.addTests(loader.loadTestsFromName(__name__ + ".test_fast"))
                    return tests
            """,
            "tests/pkg/test_fast.py": """
                import unittest

                class Fast(unittest.TestCase):
                    def test_fast(self):
                        pass
            """,
            "tests/pkg/test_slow.py": """
                import unittest

                class Slow(unittest.TestCase):
                    def test_slow(self):
                        self.fail("slow path broken")
            """,
        }
    )
    # Discovery: the package's load_tests leaves test_slow out.
    refined = suite.refine(["tests/pkg/test_slow.py"])
    assert refined["cases"] == []
    # Named on the command line, it is a module alone.
    context = suite.context(named=[str(suite.root / "tests/pkg/test_slow.py")])
    refined = suite.refine(["tests/pkg/test_slow.py"], context=context)
    assert [c["id"] for c in refined["cases"]] == ["tests/pkg/test_slow.py::Slow::test_slow"]
    run = suite.run(["tests/pkg/test_slow.py::Slow::test_slow"], context=context)
    assert run.outcomes == {"tests/pkg/test_slow.py::Slow::test_slow": "failed"}
    assert run.status == 1
    assert suite.unittest("tests/pkg/test_slow.py").returncode == 1


LOAD_TESTS_PACKAGE = {
    "tests/__init__.py": "",
    "tests/pkg/__init__.py": """
        def load_tests(loader, tests, pattern):
            tests.addTests(loader.loadTestsFromName(__name__ + ".test_fast"))
            return tests
    """,
    "tests/pkg/test_fast.py": """
        import unittest

        class Fast(unittest.TestCase):
            def test_fast(self):
                pass
    """,
    "tests/pkg/test_slow.py": """
        import unittest

        class Slow(unittest.TestCase):
            def test_slow(self):
                self.fail("slow path broken")
    """,
}


def test_a_package_and_a_file_in_it_both_named_run_its_tests_once(suite):
    # `aye tests/pkg tests/pkg/test_fast.py`: the package's load_tests loads
    # test_fast's tests, which the file named alone loads too. One run, under
    # the file's id, as pytest dedups the two arguments (and as aye does
    # without load_tests); unittest, given both names, runs it twice.
    suite.write(LOAD_TESTS_PACKAGE)
    fast = str(suite.root / "tests/pkg/test_fast.py")
    files = ["tests/pkg/__init__.py", "tests/pkg/test_fast.py", "tests/pkg/test_slow.py"]
    refined = suite.refine(files, context=suite.context(named=[fast]))
    assert [c["id"] for c in refined["cases"]] == ["tests/pkg/test_fast.py::Fast::test_fast"]
    # The files without tests say why, rather than collection's "has none".
    notes = {d["path"]: (d["severity"], d["code"]) for d in refined["diagnostics"]}
    assert notes == {
        "tests/pkg/__init__.py": ("info", "ignored"),
        "tests/pkg/test_slow.py": ("info", "ignored"),
    }
    # Without the file named, the package's discovery runs it, under the
    # package, and says where test_fast's tests went.
    refined = suite.refine(files)
    assert [c["id"] for c in refined["cases"]] == ["tests/pkg/__init__.py::Fast::test_fast"]
    messages = {d["path"]: d["message"] for d in refined["diagnostics"]}
    assert set(messages) == {"tests/pkg/test_fast.py", "tests/pkg/test_slow.py"}
    assert messages["tests/pkg/test_fast.py"].endswith("run under tests/pkg/__init__.py")
    assert messages["tests/pkg/test_slow.py"].endswith("loads none from this file")
    # A package that fails to load says so once; its files say nothing more.
    broken = (
        "def load_tests(loader, tests, pattern):\n    return tests\n\nraise RuntimeError('x')\n"
    )
    suite.write({"tests/pkg/__init__.py": broken})
    refined = suite.refine(files)
    assert refined["cases"] == []
    assert [(d["code"], d["path"]) for d in refined["diagnostics"]] == [
        ("collection-error", "tests/pkg/__init__.py")
    ]
    suite.write(LOAD_TESTS_PACKAGE)
    # A named file the package leaves out runs beside it.
    slow = str(suite.root / "tests/pkg/test_slow.py")
    refined = suite.refine(files, context=suite.context(named=[slow]))
    assert [c["id"] for c in refined["cases"]] == [
        "tests/pkg/__init__.py::Fast::test_fast",
        "tests/pkg/test_slow.py::Slow::test_slow",
    ]


def test_a_class_is_named_by_the_name_it_is_bound_to(suite):
    suite.write(
        {
            "shared.py": """
                import unittest

                class SharedTests(unittest.TestCase):
                    def test_shared(self):
                        pass
            """,
            "test_api.py": "from shared import SharedTests as TestShared\n",
            "test_backends.py": """
                import unittest

                def make_suite(backend):
                    class BackendTests(unittest.TestCase):
                        def test_roundtrip(self):
                            print(backend)
                    return BackendTests

                TestMemory = make_suite("memory")
                TestDisk = make_suite("disk")
            """,
            "test_alias.py": """
                import unittest

                class Outer:
                    class Inner(unittest.TestCase):
                        def test_inner(self):
                            pass

                Inner = Outer.Inner
                Twice = Inner
            """,
        }
    )
    refined = suite.refine(["test_api.py", "test_backends.py", "test_alias.py"])
    assert [c["id"] for c in refined["cases"]] == [
        "test_api.py::TestShared::test_shared",
        "test_backends.py::TestDisk::test_roundtrip",
        "test_backends.py::TestMemory::test_roundtrip",
        "test_alias.py::Inner::test_inner",
        "test_alias.py::Twice::test_inner",
    ]
    run = suite.run(
        [
            "test_backends.py::TestDisk::test_roundtrip",
            "test_backends.py::TestMemory::test_roundtrip",
        ]
    )
    assert run.diagnostics == []
    assert {i: e["output"]["stdout"] for i, e in run.finished.items()} == {
        "test_backends.py::TestDisk::test_roundtrip": "disk\n",
        "test_backends.py::TestMemory::test_roundtrip": "memory\n",
    }
