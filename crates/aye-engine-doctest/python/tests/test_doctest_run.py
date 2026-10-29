"""Batches: doctests run natively, with pytest's verdicts."""

import math

import pytest

OUTCOMES = '''
"""Outcomes.

>>> add(1, 2)
3
"""
import sys


def add(a, b):
    """
    >>> add(1, 2)
    3
    >>> add(1, 1)
    3
    >>> add(2, 2)
    4
    """
    return a + b


def boom():
    """
    >>> boom()
    Traceback (most recent call last):
    ...
    ValueError: boom
    >>> helper()
    """
    raise ValueError("boom")


def helper():
    """
    >>> 1
    1
    """
    raise KeyError("deep")


def all_skipped():
    """
    >>> 1  # doctest: +SKIP
    2
    >>> 3  # doctest: +SKIP
    4
    """


def some_skipped():
    """
    >>> 1  # doctest: +SKIP
    2
    >>> 3
    3
    """


def ellipsis():
    """
    >>> list(range(20))
    [0, 1, ..., 19]
    """


def whitespace():
    """
    >>> print("a   b")  # doctest: +NORMALIZE_WHITESPACE
    a b
    """


def literals():
    """
    >>> b"x"  # doctest: +ALLOW_BYTES
    'x'
    >>> 3.14159  # doctest: +NUMBER
    3.14
    >>> "y"  # doctest: +ALLOW_UNICODE
    u'y'
    """


def fail_fast():
    """
    >>> 1  # doctest: +FAIL_FAST
    2
    >>> 3
    4
    """


def quiet():
    """
    >>> 1  # doctest: +REPORT_ONLY_FIRST_FAILURE
    2
    >>> 3
    4
    """


def pytest_skip():
    """
    >>> import pytest
    >>> pytest.skip("not today")
    >>> 1
    2
    """


def pytest_xfail():
    """
    >>> import pytest
    >>> pytest.xfail("known")
    """


def pytest_fail():
    """
    >>> import pytest
    >>> pytest.fail("bad")
    >>> 1
    2
    """


def unittest_skip():
    """
    >>> import unittest
    >>> raise unittest.SkipTest("nope")
    """


def exits():
    """
    >>> raise SystemExit(3)
    """


def stderr():
    """
    >>> print("to stderr", file=sys.stderr)
    >>> print("to stdout")
    to stdout
    """


def globs_a():
    """
    >>> leaked = 1
    >>> leaked
    1
    """


def globs_b():
    """
    >>> leaked
    Traceback (most recent call last):
    ...
    NameError: name 'leaked' is not defined
    """
'''


def test_outcomes_are_pytests(suite):
    suite.write({"pkg/__init__.py": "", "pkg/outcomes.py": OUTCOMES})
    reply = suite.refine(["pkg/outcomes.py"])
    ids = [case["id"] for case in reply["cases"]]
    run = suite.run(ids)
    theirs = suite.pytest_outcomes(
        "-p", "no:python", "--doctest-modules", "-p", "no:cacheprovider", "pkg/outcomes.py"
    )
    assert run.outcomes == theirs
    assert run.started == ids
    assert run.status == 1
    assert run.outcomes["pkg/outcomes.py::pkg.outcomes.add"] == "failed"
    assert run.outcomes["pkg/outcomes.py::pkg.outcomes.all_skipped"] == "skipped"
    assert run.outcomes["pkg/outcomes.py::pkg.outcomes.pytest_xfail"] == "expected_failure"
    assert run.outcomes["pkg/outcomes.py::pkg.outcomes.unittest_skip"] == "failed"
    assert run.outcomes["pkg/outcomes.py::pkg.outcomes.globs_b"] == "passed"


def test_every_example_is_a_sub_outcome(suite):
    suite.write({"pkg/__init__.py": "", "pkg/outcomes.py": OUTCOMES})
    prefix = "pkg/outcomes.py::pkg.outcomes."
    run = suite.run(
        [
            prefix + name
            for name in (
                "add",
                "boom",
                "all_skipped",
                "some_skipped",
                "fail_fast",
                "quiet",
                "pytest_skip",
                "pytest_fail",
            )
        ]
    )
    # Every example runs, even after a failure.
    assert run.subs(prefix + "add") == [
        ("line 11: >>> add(1, 2)", "passed"),
        ("line 13: >>> add(1, 1)", "failed"),
        ("line 15: >>> add(2, 2)", "passed"),
    ]
    assert run.subs(prefix + "boom") == [
        ("line 23: >>> boom()", "passed"),
        ("line 27: >>> helper()", "failed"),
    ]
    assert run.subs(prefix + "all_skipped") == [
        ("line 42: >>> 1  # doctest: +SKIP", "skipped"),
        ("line 44: >>> 3  # doctest: +SKIP", "skipped"),
    ]
    assert run.finished[prefix + "all_skipped"]["reason"] == "all tests skipped by +SKIP option"
    assert run.subs(prefix + "some_skipped") == [
        ("line 51: >>> 1  # doctest: +SKIP", "skipped"),
        ("line 53: >>> 3", "passed"),
    ]
    assert run.outcomes[prefix + "some_skipped"] == "passed"
    # FAIL_FAST stops the test; REPORT_ONLY_FIRST_FAILURE quiets nothing.
    assert [o for _, o in run.subs(prefix + "fail_fast")] == ["failed"]
    assert [o for _, o in run.subs(prefix + "quiet")] == ["failed", "failed"]
    assert len(run.finished[prefix + "quiet"]["failures"]) == 2
    # pytest's skip and fail end the test where they're raised.
    assert [o for _, o in run.subs(prefix + "pytest_skip")] == ["passed", "skipped"]
    assert run.finished[prefix + "pytest_skip"]["reason"] == "not today"
    assert [o for _, o in run.subs(prefix + "pytest_fail")] == ["passed", "failed"]
    failure = run.failure(prefix + "pytest_fail")
    assert failure["message"] == "Failed: bad"
    assert failure["location"] == {"path": "pkg/outcomes.py", "line": 120}


def test_a_mismatch_is_a_structured_failure(suite):
    suite.write({"pkg/__init__.py": "", "pkg/outcomes.py": OUTCOMES})
    test_id = "pkg/outcomes.py::pkg.outcomes.add"
    run = suite.run([test_id])
    finished = run.finished[test_id]
    assert len(finished["failures"]) == 1
    failure = finished["failures"][0]
    assert failure["phase"] == "test"
    assert failure["exception"] == "DocTestFailure"
    assert failure["message"] == "add(1, 1): expected '3', got '2'"
    assert failure["assertion"] == {
        "op": "==",
        "left": "2",
        "right": "3",
        "diff": ["@@ -1 +1 @@", "-3", "+2"],
        "expected": "right",
        "left_source": "add(1, 1)",
        "right_source": "expected output",
        "compare": {
            "kind": "str",
            "summary": "strings differ at line 1, column 1",
            "differs": [{"at": "line 1, column 1", "left": "'2'", "right": "'3'"}],
        },
    }
    assert failure["location"] == {"path": "pkg/outcomes.py", "line": 13}
    assert failure["frames"] == [
        {
            "path": "pkg/outcomes.py",
            "line": 13,
            "function": "pkg.outcomes.add",
            "source": ">>> add(1, 1)",
        }
    ]
    assert failure["detail"].startswith(
        'File "pkg/outcomes.py", line 13, in pkg.outcomes.add\nFailed example:\n    add(1, 1)\n'
    )
    assert "Expected:\n    3\nGot:\n    2\n" in failure["detail"]
    assert failure["signature"]
    sub = next(e for e in run.of("sub_outcome") if e["outcome"] == "failed")
    assert sub["failure"] == failure


def test_an_unexpected_exception_maps_the_examples_frame(suite):
    suite.write({"pkg/__init__.py": "", "pkg/outcomes.py": OUTCOMES})
    test_id = "pkg/outcomes.py::pkg.outcomes.boom"
    failure = suite.run([test_id]).failure(test_id)
    assert failure["exception"] == "KeyError"
    assert failure["message"] == "KeyError: 'deep'"
    assert failure["frames"] == [
        {
            "path": "pkg/outcomes.py",
            "line": 27,
            "function": "pkg.outcomes.boom",
            "source": ">>> helper()",
        },
        {
            "path": "pkg/outcomes.py",
            "line": 37,
            "function": "helper",
            "source": 'raise KeyError("deep")',
        },
    ]
    assert failure["location"] == {"path": "pkg/outcomes.py", "line": 37}
    assert "Exception raised:\n    Traceback (most recent call last):" in failure["detail"]
    assert "doctest.py" not in failure["detail"]


def test_long_diffs_follow_the_report_choice(suite):
    suite.write(
        {
            "m.py": '''
def f():
    """
    >>> for i in range(4): print(i)
    0
    1
    9
    3
    """
'''
        }
    )
    run = suite.run(["m.py::m.f"])
    failure = run.failure("m.py::m.f")
    assert failure["assertion"]["diff"] == ["@@ -1,4 +1,4 @@", " 0", " 1", "-9", "+2", " 3"]
    assert "unified diff with -expected +actual" in failure["detail"]
    run = suite.run(["m.py::m.f"], context=suite.context(report="ndiff"))
    assert "ndiff with -expected +actual" in run.failure("m.py::m.f")["detail"]


def test_where_output_first_differs_only_when_it_had_to_match_exactly(suite):
    suite.write(
        {
            "m.py": '''
def lines():
    r"""
    >>> print("one\\ntwo\\nthree")
    one
    too
    three
    """


def ellipsis():
    """
    >>> print(list(range(9)))
    [0, ..., 7]
    """
'''
        }
    )
    run = suite.run(["m.py::m.lines", "m.py::m.ellipsis"])
    assertion = run.failure("m.py::m.lines")["assertion"]
    assert assertion["compare"] == {
        "kind": "str",
        "summary": "strings differ at line 2, column 2",
        "differs": [{"at": "line 2, column 2", "left": "'two'", "right": "'too'"}],
    }
    # Under ELLIPSIS, an expected ``...`` matches what differs from it.
    assertion = run.failure("m.py::m.ellipsis")["assertion"]
    assert (assertion["expected"], assertion["left_source"]) == ("right", "print(list(range(9)))")
    assert "compare" not in assertion


def test_option_flags_come_from_the_context(suite):
    suite.write({"m.py": 'def f():\n    """\n    >>> print("a   b")\n    a b\n    """\n'})
    assert suite.run(["m.py::m.f"]).outcomes == {"m.py::m.f": "failed"}
    context = suite.context(optionflags=["NORMALIZE_WHITESPACE"])
    assert suite.run(["m.py::m.f"], context=context).outcomes == {"m.py::m.f": "passed"}
    # ELLIPSIS is pytest's default; without it, `...` is literal.
    suite.write({"e.py": 'def f():\n    """\n    >>> list(range(9))\n    [0, ..., 8]\n    """\n'})
    assert suite.run(["e.py::e.f"]).outcomes == {"e.py::e.f": "passed"}
    context = suite.context(optionflags=[])
    assert suite.run(["e.py::e.f"], context=context).outcomes == {"e.py::e.f": "failed"}


def test_without_pytest(suite):
    suite.write({"pkg/__init__.py": "", "pkg/outcomes.py": OUTCOMES})
    prefix = "pkg/outcomes.py::pkg.outcomes."
    names = ["add", "boom", "all_skipped", "ellipsis", "whitespace", "literals", "globs_b"]
    run = suite.run([prefix + n for n in names], no_pytest=True)
    assert run.pytest_imported is False
    assert run.outcomes == {
        prefix + "add": "failed",
        prefix + "boom": "failed",
        prefix + "all_skipped": "skipped",
        prefix + "ellipsis": "passed",
        prefix + "whitespace": "passed",
        prefix + "literals": "passed",
        prefix + "globs_b": "passed",
    }


def test_text_files(suite):
    suite.write(
        {
            "docs/test_guide.txt": (
                "A guide.\n\n>>> x = 2\n>>> x * 2\n4\n\nMore prose.\n\n>>> x + 1\n4\n"
            ),
        }
    )
    test_id = "docs/test_guide.txt::test_guide.txt"
    run = suite.run([test_id])
    assert run.outcomes == {test_id: "failed"}
    assert run.subs(test_id) == [
        ("line 3: >>> x = 2", "passed"),
        ("line 4: >>> x * 2", "passed"),
        ("line 9: >>> x + 1", "failed"),
    ]
    assert run.failure(test_id)["location"] == {"path": "docs/test_guide.txt", "line": 9}


def test_capture_takes_stderr(suite):
    suite.write({"pkg/__init__.py": "", "pkg/outcomes.py": OUTCOMES})
    test_id = "pkg/outcomes.py::pkg.outcomes.stderr"
    run = suite.run([test_id])
    assert run.outcomes == {test_id: "passed"}
    assert run.finished[test_id]["output"] == {"stderr": "to stderr\n"}
    run = suite.run([test_id], policy={"capture": False})
    assert "output" not in run.finished[test_id]
    assert "to stderr" in run.stderr


def test_maxfail_stops_the_batch(suite):
    suite.write({"pkg/__init__.py": "", "pkg/outcomes.py": OUTCOMES})
    prefix = "pkg/outcomes.py::pkg.outcomes."
    run = suite.run([prefix + "add", prefix + "boom", prefix + "ellipsis"], policy={"maxfail": 1})
    assert run.started == [prefix + "add"]
    assert run.status == 1


def test_files_that_fail_to_load(suite):
    suite.write(
        {
            "broken.py": "import nonexistent_module_xyz\n",
            "fine.py": 'def f():\n    """\n    >>> 1\n    1\n    """\n',
            "skipped.py": "import unittest\nraise unittest.SkipTest('later')\n",
        }
    )
    run = suite.run(
        [
            "broken.py::broken.f",
            "fine.py::fine.f",
            "fine.py::fine.gone",
            "skipped.py::skipped.f",
            "missing.py::missing.f",
        ]
    )
    assert run.outcomes == {"fine.py::fine.f": "passed", "skipped.py::skipped.f": "skipped"}
    assert run.finished["skipped.py::skipped.f"]["reason"] == "later"
    codes = sorted((d["path"], d["code"], d["severity"]) for d in run.diagnostics)
    assert codes == [
        ("broken.py", "collection-error", "error"),
        ("fine.py", "not-found", "error"),
        ("missing.py", "not-found", "error"),
    ]
    assert run.status == 1


def test_a_keyboard_interrupt_stops_the_batch(suite):
    suite.write(
        {
            "m.py": (
                'def a():\n    """\n    >>> raise KeyboardInterrupt\n    """\n'
                'def b():\n    """\n    >>> 1\n    1\n    """\n'
            )
        }
    )
    run = suite.run(["m.py::m.a", "m.py::m.b"])
    assert run.status == 2
    assert run.outcomes == {"m.py::m.a": "error"}
    stopped = [d for d in run.diagnostics if d["code"] == "batch-stopped"]
    assert len(stopped) == 1
    assert stopped[0]["severity"] == "error"
    assert "1 of 2 tests in the batch never started: m.py::m.b" in stopped[0]["message"]


def test_the_import_name_clash_is_pytests(suite):
    body = 'def f():\n    """\n    >>> 1\n    1\n    """\n'
    suite.write({"a/utils.py": body, "b/utils.py": body})
    run = suite.run(["a/utils.py::utils.f", "b/utils.py::utils.f"])
    assert run.outcomes == {"a/utils.py::utils.f": "passed"}
    assert run.diagnostics[0]["code"] == "collection-error"
    assert "import file mismatch" in run.diagnostics[0]["message"]


def test_the_context_service_names_the_test(suite):
    suite.write(
        {
            "m.py": (
                "import aye_engine\n"
                'def f():\n    """\n'
                "    >>> import sys\n"
                "    >>> sys.modules['aye_engine'] is not None\n"
                "    True\n"
                '    """\n'
            )
        }
    )
    assert suite.run(["m.py::m.f"]).outcomes == {"m.py::m.f": "passed"}


@pytest.mark.parametrize("no_pytest", [False, True])
def test_every_record_is_json(suite, no_pytest):
    suite.write(
        {
            "m.py": (
                'def f():\n    """\n    >>> print("\\\\udcff")\n    x\n    """\n'
                'def g():\n    """\n    >>> float("nan")\n    0\n    """\n'
            )
        }
    )
    run = suite.run(["m.py::m.f", "m.py::m.g"], no_pytest=no_pytest)
    assert run.outcomes == {"m.py::m.f": "failed", "m.py::m.g": "failed"}


def test_a_bad_outcome_carries_its_rerun(suite):
    suite.write({"pkg/__init__.py": "", "pkg/outcomes.py": OUTCOMES})
    prefix = "pkg/outcomes.py::pkg.outcomes."
    args = ["--doctest-glob=*.rst", "--junitxml", "r.xml", "--junit-xml=s.xml", "-v"]
    run = suite.run([prefix + "add", prefix + "ellipsis"], policy={"framework_args": args})
    assert run.finished[prefix + "add"]["reproduce"] == {
        "command": ["aye", prefix + "add", "--", "--doctest-glob=*.rst", "-v"]
    }
    assert "reproduce" not in run.finished[prefix + "ellipsis"]
    plain = suite.run([prefix + "add"])
    assert plain.finished[prefix + "add"]["reproduce"] == {"command": ["aye", prefix + "add"]}
    # C26: aye's own options that shaped the run come before the id.
    configured = suite.run(
        [prefix + "add"],
        policy={"framework_args": args},
        context=suite.context(rerun_args=["-c", "alt.ini"]),
    )
    assert configured.finished[prefix + "add"]["reproduce"] == {
        "command": ["aye", "-c", "alt.ini", prefix + "add", "--", "--doctest-glob=*.rst", "-v"]
    }


PATCHING = '''
def a():
    """
    >>> import io, time
    >>> from unittest import mock
    >>> time.perf_counter = lambda: float("nan")
    >>> io.StringIO = None
    >>> for target in (
    ...     "os.path.abspath",
    ...     "os.path.relpath",
    ...     "os.path.dirname",
    ...     "os.path.join",
    ...     "os.path.isabs",
    ...     "os.path.normpath",
    ...     "os.getcwd",
    ...     "traceback.extract_tb",
    ...     "traceback.format_list",
    ...     "math.isfinite",
    ...     "hashlib.sha1",
    ... ):
    ...     _ = mock.patch(target, mock.MagicMock(side_effect=RuntimeError("no"))).start()
    """


def b():
    """
    >>> print("x")
    y
    """


def c():
    """
    >>> 1 / 0
    """


def d():
    """
    >>> import sys
    >>> print("to stderr", file=sys.stderr)
    """
'''


@pytest.mark.parametrize("no_pytest", [False, True])
def test_examples_that_patch_the_standard_library_change_nothing_reported(suite, no_pytest):
    # An example may patch what the executor uses and leave it patched for
    # the doctests after it: each is still reported, in full.
    suite.write({"m.py": PATCHING})
    run = suite.run(["m.py::m.a", "m.py::m.b", "m.py::m.c", "m.py::m.d"], no_pytest=no_pytest)
    assert run.outcomes == {
        "m.py::m.a": "passed",
        "m.py::m.b": "failed",
        "m.py::m.c": "failed",
        "m.py::m.d": "passed",
    }
    mismatch = run.failure("m.py::m.b")
    assert mismatch["location"] == {"path": "m.py", "line": 26}
    assert mismatch["assertion"]["left"] == "x"
    raised = run.failure("m.py::m.c")
    assert raised["message"] == "ZeroDivisionError: division by zero"
    assert raised["location"] == {"path": "m.py", "line": 33}
    assert run.finished["m.py::m.d"]["output"] == {"stderr": "to stderr\n"}
    for event in run.events:
        assert math.isfinite(event["t_ms"])
        if "duration_ms" in event:
            assert math.isfinite(event["duration_ms"]) and event["duration_ms"] >= 0


def test_a_failure_before_a_skip_or_xfail_fails_the_test(suite):
    suite.write(
        {
            "mod.py": '''
                def a():
                    """
                    >>> 1
                    2
                    >>> import pytest; pytest.skip("later")
                    """

                def b():
                    """
                    >>> 1
                    2
                    >>> import pytest; pytest.xfail("later")
                    """

                def c():
                    """
                    >>> raise ValueError("x")
                    >>> import pytest; pytest.skip("later")
                    """

                def d():
                    """
                    >>> import pytest; pytest.skip("first")
                    >>> 1
                    2
                    """
            '''
        }
    )
    ids = [f"mod.py::mod.{name}" for name in "abcd"]
    run = suite.run(ids)
    expected = {
        "mod.py::mod.a": "failed",
        "mod.py::mod.b": "failed",
        "mod.py::mod.c": "failed",
        "mod.py::mod.d": "skipped",
    }
    assert run.outcomes == expected
    assert run.status == 1
    assert run.failure("mod.py::mod.a")["message"] == "1: expected '2', got '1'"
    assert suite.pytest_outcomes("--doctest-modules", "mod.py") == expected
    # With --doctest-continue-on-failure, pytest lets the later outcome end
    # the test.
    context = suite.context(continue_on_failure=True)
    assert suite.run(ids, context=context).outcomes == {
        "mod.py::mod.a": "skipped",
        "mod.py::mod.b": "expected_failure",
        "mod.py::mod.c": "skipped",
        "mod.py::mod.d": "skipped",
    }


@pytest.mark.parametrize("flag", ["COMPARISON_FLAGS", "REPORTING_FLAGS"])
def test_combined_option_flags(suite, flag):
    suite.write({"m.py": 'def ws():\n    """\n    >>> list(range(3))\n    [0,  1, 2]\n    """\n'})
    context = suite.context(optionflags=[flag])
    run = suite.run(["m.py::m.ws"], context=context)
    assert run.diagnostics == []
    # COMPARISON_FLAGS holds SKIP: doctest skips every example, and pytest
    # (which skips a test only for its examples' own +SKIP) passes it.
    assert run.outcomes == {"m.py::m.ws": "passed" if flag == "COMPARISON_FLAGS" else "failed"}
    refined = suite.refine(["m.py"], context=context)
    assert [c["id"] for c in refined["cases"]] == ["m.py::m.ws"]
    if flag == "COMPARISON_FLAGS":
        suite.write({"pytest.ini": "[pytest]\ndoctest_optionflags = COMPARISON_FLAGS\n"})
        assert suite.pytest_outcomes("--doctest-modules", "m.py") == {"m.py::m.ws": "passed"}


def test_failures_without_a_line_are_not_grouped(suite):
    suite.write(
        {
            "m.py": '__test__ = {"arith": """\n>>> 1 + 1\n3\n>>> 2 + 2\n5\n"""}\n',
            "m1.py": '__test__ = {"a": """\n>>> print(1)\n2\n"""}\n',
            "m2.py": '__test__ = {"b": """\n>>> print(3)\n4\n"""}\n',
        }
    )
    ids = ["m.py::m.__test__.arith", "m1.py::m1.__test__.a", "m2.py::m2.__test__.b"]
    run = suite.run(ids)
    failed = [e for e in run.of("sub_outcome") if e["outcome"] == "failed"]
    assert len(failed) == 4
    assert all("location" not in e["failure"] for e in failed)
    signatures = [e["failure"]["signature"] for e in failed]
    assert len(set(signatures)) == 4
    # The same example failing the same way twice still groups.
    again = suite.run(ids)
    assert [
        e["failure"]["signature"] for e in again.of("sub_outcome") if e["outcome"] == "failed"
    ] == signatures


def test_the_diff_takes_blanklines_as_doctest_does(suite):
    suite.write(
        {
            "m.py": '''
                def f():
                    """
                    >>> print("header\\\\n\\\\nline one\\\\nline two")
                    header
                    <BLANKLINE>
                    line one
                    line 2
                    """

                def g():
                    """
                    >>> print("a\\\\n\\\\nb")  # doctest: +DONT_ACCEPT_BLANKLINE
                    a
                    <BLANKLINE>
                    b
                    """
            '''
        }
    )
    run = suite.run(["m.py::m.f", "m.py::m.g"])
    assert run.failure("m.py::m.f")["assertion"]["diff"] == [
        "@@ -2,3 +2,3 @@",
        " <BLANKLINE>",
        " line one",
        "-line 2",
        "+line two",
    ]
    assert "-<BLANKLINE>" in run.failure("m.py::m.g")["assertion"]["diff"]


def test_examples_see_a_pytest_session(suite):
    suite.write(
        {
            "m.py": '''
                import os

                AT_IMPORT = os.environ.get("PYTEST_VERSION")

                def f():
                    """
                    >>> AT_IMPORT == os.environ["PYTEST_VERSION"] != ""
                    True
                    >>> os.environ["PYTEST_CURRENT_TEST"]
                    'm.py::m.f (call)'
                    """
            '''
        }
    )
    run = suite.run(["m.py::m.f"])
    assert run.outcomes == {"m.py::m.f": "passed"}, run.finished
    assert suite.pytest_outcomes("--doctest-modules", "m.py") == {"m.py::m.f": "passed"}
    # Without pytest, there is no pytest session to see.
    suite.write(
        {
            "n.py": 'import os\n\ndef f():\n    """\n    >>> "PYTEST_VERSION" in os.environ\n    False\n    """\n'
        }
    )
    assert suite.run(["n.py::n.f"], no_pytest=True).outcomes == {"n.py::n.f": "passed"}


def test_an_example_that_closes_a_captured_stream_loses_nothing(suite):
    suite.write(
        {
            "pkg/__init__.py": "",
            "pkg/a.py": '''
                def f1():
                    """
                    >>> 1 + 1
                    2
                    """

                def f2():
                    """
                    >>> import sys
                    >>> print("to stderr", file=sys.stderr)
                    >>> sys.stderr.close()
                    """

                def f3():
                    """
                    >>> 3
                    4
                    """

                def f4():
                    """
                    >>> import sys
                    >>> sys.stderr.buffer.write(b"bytes\\\\n") and None
                    >>> sys.stderr.encoding
                    'utf-8'
                    >>> isinstance(sys.stderr.fileno(), int)
                    True
                    """
            ''',
            "pkg/b.py": '''
                def g():
                    """
                    >>> 'b'
                    'b'
                    """
            ''',
        }
    )
    ids = [f"pkg/a.py::pkg.a.f{i}" for i in range(1, 5)] + ["pkg/b.py::pkg.b.g"]
    run = suite.run(ids)
    assert run.outcomes == {
        "pkg/a.py::pkg.a.f1": "passed",
        "pkg/a.py::pkg.a.f2": "passed",
        "pkg/a.py::pkg.a.f3": "failed",
        "pkg/a.py::pkg.a.f4": "passed",
        "pkg/b.py::pkg.b.g": "passed",
    }
    assert run.status == 1
    assert run.finished["pkg/a.py::pkg.a.f2"]["output"] == {"stderr": "to stderr\n"}
    assert run.finished["pkg/a.py::pkg.a.f4"]["output"] == {"stderr": "bytes\n"}


def test_an_example_leaving_os_path_patched_breaks_no_later_batch(suite):
    suite.write(
        {
            "pkg/__init__.py": "",
            "pkg/z.py": '''
                def patch():
                    """
                    >>> import os.path
                    >>> os.path.join = lambda *parts: "/nowhere"
                    """
            ''',
            "pkg/c.py": '''
                import os.path

                # The real join, whatever the platform's separator: an
                # earlier batch's patch would make it "/nowhere".
                HERE = os.path.join("a", "b") == os.sep.join(["a", "b"])

                def f():
                    """
                    >>> HERE
                    True
                    >>> 1
                    2
                    """
            ''',
        }
    )
    first = suite.batch(["pkg/z.py::pkg.z.patch"])
    second = suite.batch(["pkg/c.py::pkg.c.f"], id=4)
    replies, _ = suite._call({"batches": [first, second], "no_pytest": False})
    later = replies[1]
    assert [e for e in later["events"] if e["event"] == "diagnostic"] == []
    outcomes = {e["id"]: e["outcome"] for e in later["events"] if e["event"] == "test_finished"}
    assert outcomes == {"pkg/c.py::pkg.c.f": "failed"}
    subs = [e["outcome"] for e in later["events"] if e["event"] == "sub_outcome"]
    assert subs == ["passed", "failed"]


WARNS = '''
import warnings


def f():
    """
    >>> warnings.warn("a warning")
    >>> 1
    1
    """


def g():
    """
    >>> warnings.warn("old", DeprecationWarning)
    >>> 2
    2
    """
'''


def test_pytests_warning_filters_apply(suite):
    suite.write(
        {
            "m.py": WARNS,
            "n.py": '"""\n>>> 1\n1\n"""\nimport warnings\n\nwarnings.warn("deprecated module", DeprecationWarning)\n',
        }
    )
    ids = ["m.py::m.f", "m.py::m.g"]
    assert suite.run(ids).outcomes == {"m.py::m.f": "passed", "m.py::m.g": "passed"}
    errors = suite.context(filterwarnings=["error"])
    run = suite.run(ids, context=errors)
    assert run.outcomes == {"m.py::m.f": "failed", "m.py::m.g": "failed"}
    assert run.failure("m.py::m.f")["exception"] == "UserWarning"
    # A module that warns while imported fails to collect.
    run = suite.run(["n.py::n"], context=errors)
    (error,) = run.diagnostics
    assert error["code"] == "collection-error"
    assert "DeprecationWarning: deprecated module" in error["message"]
    assert suite.refine(["n.py"], context=errors)["diagnostics"][0]["code"] == "collection-error"
    # -W wins over filterwarnings, and its message is a literal.
    context = suite.context(pythonwarnings=["error::DeprecationWarning"])
    run = suite.run(ids, context=context)
    assert run.outcomes == {"m.py::m.f": "passed", "m.py::m.g": "failed"}
    context = suite.context(filterwarnings=["error"], pythonwarnings=["ignore::UserWarning"])
    assert suite.run(ids, context=context).outcomes == {
        "m.py::m.f": "passed",
        "m.py::m.g": "failed",
    }
    # A filter pytest would refuse is left out.
    context = suite.context(filterwarnings=["error::NoSuchWarning", "bogus"])
    assert suite.run(ids, context=context).outcomes == {
        "m.py::m.f": "passed",
        "m.py::m.g": "passed",
    }


def test_conftests_are_imported_before_the_modules_below_them(suite):
    suite.write(
        {
            "conftest.py": """
                import os
                import sys

                sys.path.insert(0, os.path.join(os.path.dirname(__file__), "lib"))
                os.environ["APP_MODE"] = "test"
            """,
            "lib/helper.py": "def double(x):\n    return 2 * x\n",
            "app/core.py": '''
                import os

                from helper import double

                MODE = os.environ.get("APP_MODE", "prod")


                def quad(x):
                    """
                    >>> quad(2)
                    8
                    >>> MODE
                    'test'
                    """
                    return double(double(x))
            ''',
            "app/sub/conftest.py": "import os\nos.environ['SUB'] = 'yes'\n",
            "app/sub/deep.py": '''
                import os

                def f():
                    """
                    >>> os.environ["SUB"]
                    'yes'
                    """
            ''',
            "other/conftest.py": "raise RuntimeError('broken conftest')\n",
            "other/mod.py": 'def f():\n    """\n    >>> 1\n    1\n    """\n',
        }
    )
    conftests = {"initial": [str(suite.root / "conftest.py")], "confcutdir": str(suite.root)}
    context = suite.context(conftests=conftests)
    run = suite.run(["app/core.py::core.quad", "app/sub/deep.py::deep.f"], context=context)
    assert run.diagnostics == []
    assert run.outcomes == {"app/core.py::core.quad": "passed", "app/sub/deep.py::deep.f": "passed"}
    run = suite.run(["other/mod.py::mod.f"], context=context)
    (error,) = run.diagnostics
    assert error["code"] == "collection-error"
    assert "ImportError while loading conftest" in error["message"]
    assert "broken conftest" in error["message"]
    # Without the context's conftests (--noconftest), none is imported.
    run = suite.run(["app/core.py::core.quad"])
    assert run.diagnostics[0]["code"] == "collection-error"
    # Nor without pytest, whose plugins they are.
    run = suite.run(["app/sub/deep.py::deep.f"], context=context, no_pytest=True)
    assert run.outcomes == {"app/sub/deep.py::deep.f": "failed"}


DOC = 'def f():\n    """\n    >>> 1\n    1\n    """\n'


@pytest.mark.parametrize(
    "skips",
    [
        "import pytest\npytest.importorskip('aye_missing_zz')\n",
        "import pytest\npytest.skip('no backend')\n",
        "import pytest\npytest.skip('no backend', allow_module_level=True)\n",
    ],
)
def test_a_conftest_that_skips_skips_the_files_below_it(suite, skips):
    # A regression from importing conftests: pytest skips the directory
    # ('1 passed, 1 skipped'), where its skip was an import error.
    suite.write(
        {
            "pyproject.toml": '[tool.pytest.ini_options]\naddopts = "--doctest-modules"\n',
            "top.py": DOC,
            "sub/conftest.py": skips,
            "sub/m.py": DOC,
            "sub/n.py": "import os\n",
            # Below the skipped directory pytest loads no conftest.
            "sub/deeper/conftest.py": "raise RuntimeError('never loaded')\n",
            "sub/deeper/d.py": DOC,
        }
    )
    reason = "could not import 'aye_missing_zz'" if "importorskip" in skips else "no backend"
    conftests = {"initial": [], "confcutdir": str(suite.root)}
    context = suite.context(conftests=conftests)
    ids = ["top.py::top.f", "sub/m.py::m.f", "sub/deeper/d.py::d.f"]
    run = suite.run(ids, context=context)
    assert run.diagnostics == []
    assert run.outcomes == {
        "top.py::top.f": "passed",
        "sub/m.py::m.f": "skipped",
        "sub/deeper/d.py::d.f": "skipped",
    }
    assert run.finished["sub/m.py::m.f"]["reason"].startswith(reason)
    assert run.status == 0
    # Refinement says it once, on the directory, as pytest's one skip.
    reply = suite.refine(["sub/conftest.py", "sub/n.py", "sub/deeper/d.py"], context=context)
    assert reply["cases"] == []
    (skipped,) = reply["diagnostics"]
    assert skipped["severity"] == "info"
    assert skipped["code"] == "skipped"
    assert skipped["path"] == "sub"
    assert skipped["message"].startswith(reason)
    # The referee.
    done = suite.pytest()
    assert "1 passed, 1 skipped" in done.stdout, done.stdout
    assert done.returncode == 0


@pytest.mark.parametrize(
    ("source", "kind", "shown"),
    [
        (
            "import aye_missing_zz\n",
            "ImportError",
            "ModuleNotFoundError: No module named 'aye_missing_zz'",
        ),
        (
            "import unittest\nraise unittest.SkipTest('not pytest')\n",
            "ImportError",
            "SkipTest: not pytest",
        ),
        # The pytest adapter's words for pytest's own outcomes.
        ("import pytest\npytest.fail('broken')\n", "Failed", "Failed: broken"),
        ("import pytest\npytest.xfail('later')\n", "Failed", "XFailed: later"),
    ],
)
def test_a_broken_conftest_is_one_error_on_its_own_path(suite, source, kind, shown):
    # pytest fails its directory once ('1 error'): one diagnostic, on the
    # conftest, with a path from the root, however many files and batches
    # load it; the files below it don't run.
    suite.write(
        {
            "pyproject.toml": '[tool.pytest.ini_options]\naddopts = "--doctest-modules"\n',
            "top.py": DOC,
            "sub/conftest.py": source,
            "sub/m.py": DOC,
            "sub/n.py": DOC,
        }
    )
    conftests = {"initial": [], "confcutdir": str(suite.root)}
    context = suite.context(conftests=conftests)
    run = suite.run(["top.py::top.f", "sub/m.py::m.f", "sub/n.py::n.f"], context=context)
    (error,) = run.diagnostics
    assert error["code"] == "collection-error"
    assert error["path"] == "sub/conftest.py"
    first, *_, last = error["message"].splitlines()
    assert first == f"{kind} while loading conftest 'sub/conftest.py'."
    assert "sub/conftest.py:" in error["message"]
    assert str(suite.root) not in error["message"]
    assert last.endswith(shown)
    assert run.outcomes == {"top.py::top.f": "passed"}
    assert run.status == 1
    # A later batch on the same worker doesn't report it again.
    first_batch = suite.batch(["sub/m.py::m.f"], context=context)
    second_batch = suite.batch(["sub/n.py::n.f"], context=context, id=4)
    replies, _ = suite._call({"batches": [first_batch, second_batch], "no_pytest": False})
    diagnostics = [[e for e in reply["events"] if e["event"] == "diagnostic"] for reply in replies]
    assert [len(d) for d in diagnostics] == [1, 0]
    assert [reply["status"] for reply in replies] == [1, 1]
    # Refinement too.
    reply = suite.refine(["sub/conftest.py", "sub/m.py"], context=context)
    assert reply["cases"] == []
    assert [(d["code"], d["path"]) for d in reply["diagnostics"]] == [
        ("collection-error", "sub/conftest.py")
    ]
    # The referee.
    done = suite.pytest()
    assert "1 error" in done.stdout, done.stdout
    assert done.returncode == 2


@pytest.mark.parametrize(
    ("source", "kind"),
    [
        ("import aye_missing_zz\n", "ImportError"),
        # The pytest adapter's words, so that the two adapters' reports of
        # the one conftest show once.
        ("import pytest\npytest.importorskip('aye_missing_zz')\n", "Skipped"),
        ("import pytest\npytest.skip('no', allow_module_level=True)\n", "Skipped"),
        ("import pytest\npytest.fail('broken')\n", "Failed"),
    ],
)
def test_a_broken_initial_conftest_is_a_usage_error(suite, source, kind):
    # One pytest loads at start-up stops it before its session: exit 4 for
    # an import error, its exception for a skip.
    suite.write(
        {
            "pyproject.toml": '[tool.pytest.ini_options]\naddopts = "--doctest-modules"\n',
            "conftest.py": source,
            "m.py": DOC,
            "tests/t.py": DOC,
        }
    )
    initial = [str(suite.root / "conftest.py")]
    context = suite.context(conftests={"initial": initial, "confcutdir": str(suite.root)})
    run = suite.run(["m.py::m.f", "tests/t.py::t.f"], context=context)
    (error,) = run.diagnostics
    assert error["code"] == "usage-error"
    assert error["path"] == "conftest.py"
    assert error["message"].startswith(f"{kind} while loading conftest 'conftest.py'.")
    assert run.outcomes == {}
    done = suite.pytest()
    assert done.returncode in (1, 4)
    shown = "ImportError while loading conftest" if kind == "ImportError" else f"{kind}: "
    assert shown in done.stderr, done.stderr


def test_the_paths_a_dotted_name_selects_choose_the_initial_conftests(suite):
    # Recheck regression 2: `aye top` selects `top.py` by name. The
    # recognizer, seeing no path, gives the initial conftests of pytest's
    # default (the current directory's and its `test*` directories'), which
    # `pytest top.py` never loads: the binary's `selected_paths` names the
    # paths the user's run starts from, and theirs are the initial ones.
    suite.write(
        {
            "pyproject.toml": '[tool.pytest.ini_options]\naddopts = "--doctest-modules"\n',
            "top.py": DOC,
            "tests/conftest.py": "import pytest\npytest.importorskip('aye_missing_zz')\n",
            "tests/m.py": DOC,
            "pkg/conftest.py": "import os\nos.environ['AYE_PKG'] = 'loaded'\n",
            "pkg/deep/d.py": 'def f():\n    """\n    >>> import os; os.environ["AYE_PKG"]\n    \'loaded\'\n    """\n',
            "pkg/testing/conftest.py": "raise RuntimeError('a test* directory of pkg')\n",
        }
    )
    initial = [str(suite.root / "tests/conftest.py")]
    conftests = {"initial": initial, "confcutdir": str(suite.root)}
    # Without it, the default's conftest stops the batch.
    run = suite.run(["top.py::top.f"], context=suite.context(conftests=conftests))
    assert [(d["code"], d["path"]) for d in run.diagnostics] == [
        ("usage-error", "tests/conftest.py")
    ]
    assert run.outcomes == {}
    selected = [str(suite.root / "top.py")]
    context = suite.context(conftests=conftests, selected_paths=selected)
    run = suite.run(["top.py::top.f"], context=context)
    assert run.diagnostics == []
    assert run.outcomes == {"top.py::top.f": "passed"}
    assert suite.pytest("top.py").returncode == 0
    # A selected file's directories' conftests are initial; a directory's
    # `test*` directories' too.
    selected = [str(suite.root / "pkg/deep/d.py")]
    context = suite.context(conftests=conftests, selected_paths=selected)
    run = suite.run(["pkg/deep/d.py::d.f"], context=context)
    assert run.diagnostics == []
    assert run.outcomes == {"pkg/deep/d.py::d.f": "passed"}
    selected = [str(suite.root / "pkg")]
    context = suite.context(conftests=conftests, selected_paths=selected)
    run = suite.run(["pkg/deep/d.py::d.f"], context=context)
    assert [(d["code"], d["path"]) for d in run.diagnostics] == [
        ("usage-error", "pkg/testing/conftest.py")
    ]
    done = suite.pytest("pkg")
    assert done.returncode == 4, done.stdout + done.stderr
    assert "ImportError while loading conftest" in done.stderr
    # Paths that don't exist leave the context's.
    context = suite.context(conftests=conftests, selected_paths=[str(suite.root / "gone.py")])
    run = suite.run(["top.py::top.f"], context=context)
    assert [d["code"] for d in run.diagnostics] == ["usage-error"]
    # And with `--noconftest` there are none to choose.
    run = suite.run(["top.py::top.f"], context=suite.context(selected_paths=selected))
    assert run.outcomes == {"top.py::top.f": "passed"}


def test_a_retried_batch_runs_only_what_did_not_run(suite):
    # The runner re-queues a crashed batch with `ran_ids`, the tests that
    # ran: a file's id stands for all its doctests, and those don't run
    # again.
    suite.write({"m.py": DOC + "\n\n" + DOC.replace("def f", "def g")})
    run = suite.run(["m.py"])
    assert run.started == ["m.py::m.f", "m.py::m.g"]
    run = suite.run(["m.py"], context=suite.context(ran_ids=["m.py::m.f"]))
    assert run.started == ["m.py::m.g"]
    assert run.outcomes == {"m.py::m.g": "passed"}
    run = suite.run(["m.py::m.f", "m.py::m.g"], context=suite.context(ran_ids=["m.py::m.g"]))
    assert run.started == ["m.py::m.f"]
