"""Tests of calc.ops. This docstring is a doctest too, run before the tests.

>>> from calc.ops import add
>>> add(1, 1)
2
"""

import pytest

from calc.ops import Counter, add, divide, mean


def double(x):
    """A helper in a test module; pytest collects its doctest.

    >>> double(2)
    4
    """
    return x * 2


def test_add(pair):
    assert add(*pair) == 3


@pytest.mark.parametrize(("a", "b", "want"), [(1, 2, 3), (2, 2, 5)])
def test_add_table(a, b, want):
    assert add(a, b) == want


def test_divide_by_zero():
    with pytest.raises(ZeroDivisionError):
        divide(1, 0)


@pytest.mark.skip(reason="not today")
def test_skipped():
    raise AssertionError


@pytest.mark.xfail(reason="mean is exact", strict=True)
def test_mean_rounds():
    assert mean([1, 2]) == 1


class TestCounter:
    def test_bump(self):
        assert Counter().bump() == 1

    def test_double(self):
        assert double(Counter().bump()) == 2
