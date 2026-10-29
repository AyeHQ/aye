import sys
import unittest

import pytest


class Base:
    def test_inherited(self):
        pass


class TestChild(Base):
    def test_own(self):
        pass


class TestOuter:
    def test_outer(self):
        pass

    class TestInner:
        def test_inner(self):
            pass


class TestHasInit:
    def __init__(self):
        pass

    def test_never_collected(self):
        pass


class Checks(unittest.TestCase):
    def test_unittest_method(self):
        self.assertEqual(1, 1)

    def test_unittest_failure(self):
        self.assertEqual(1, 2)

    def helper(self):
        pass


def test_defined_twice():
    assert False, "the first definition is shadowed"


def test_defined_twice():  # noqa: F811
    pass


if sys.version_info >= (3, 0):

    def test_conditional():
        pass


@pytest.fixture
def test_looks_like_a_test_but_is_a_fixture():
    return 1


class NotATest:
    def test_ignored(self):
        pass


class TestDisabled:
    __test__ = False

    def test_ignored(self):
        pass
