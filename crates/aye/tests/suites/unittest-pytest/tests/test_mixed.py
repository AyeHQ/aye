"""TestCase classes and pytest functions in one file.

The classes come first, in name order: aye lists a file's tests adapter by
adapter (unittest's, then pytest's) and unittest orders classes by name,
where pytest keeps the file's order.
"""

import unittest

import pytest


class AccountTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.opened = ["main"]

    def setUp(self):
        self.balance = 10
        self.addCleanup(self.opened.clear)

    def test_close(self):
        self.skipTest("closing is not supported yet")

    def test_deposit(self):
        self.balance += 5
        self.assertEqual(self.balance, 15)

    @unittest.expectedFailure
    def test_known_bug(self):
        self.assertEqual(self.balance, 11)

    @unittest.expectedFailure
    def test_fixed_bug(self):
        self.assertEqual(self.balance, 10)

    def test_overdraw(self):
        self.assertGreaterEqual(self.balance - 20, 0)

    def test_plain_assert(self):
        assert self.balance == 10

    @unittest.skip("not on this branch")
    def test_transfer(self):
        raise AssertionError


class BrokenSetupTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise RuntimeError("no database")

    def test_insert(self):
        pass

    def test_query(self):
        pass


@unittest.skipIf(True, "skipped as a class")
class SkippedClassTests(unittest.TestCase):
    def test_anything(self):
        raise AssertionError


def test_plain():
    assert 1 + 1 == 2


def test_plain_fails():
    assert [1, 2] == [1, 3]


@pytest.mark.parametrize("n", [1, 2, 3])
def test_param(n):
    assert n > 0


def test_with_fixture(ledger):
    assert ledger["opening"] == 10


@pytest.mark.xfail(reason="rounding")
def test_xfail():
    assert round(2.5) == 3


class TestPlainClass:
    def test_method(self):
        assert "abc".upper() == "ABC"
