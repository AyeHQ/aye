"""pytest's own subtests: a subtest's result is its own, and only a failed
one changes the outcome of the test holding it."""

import pytest


def test_subtests_pass(subtests):
    for i in range(2):
        with subtests.test("even", i=i):
            assert i < 2


def test_subtest_skipped(subtests):
    with subtests.test("skipped"):
        pytest.skip("not this one")


def test_subtest_expected_failure(subtests):
    with subtests.test("known bug"):
        pytest.xfail("known bug")


def test_subtest_fails(subtests):
    for i in range(3):
        with subtests.test("case", i=i):
            assert i != 1
