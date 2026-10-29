"""Imported to be known: the decorator may replace the function."""

import functools


@functools.cache
def fib(n):
    """
    >>> fib(10)
    55
    """
    return n if n < 2 else fib(n - 1) + fib(n - 2)


def plain():
    """
    >>> plain()
    'plain'
    """
    return "plain"
