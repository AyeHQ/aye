"""Arithmetic, with doctests that pass, fail and are skipped."""


def add(a, b):
    """Add two things.

    >>> add(1, 2)
    3
    >>> add("a", "b")
    'ab'
    """
    return a + b


def divide(a, b):
    """Divide, refusing zero.

    >>> divide(6, 3)
    2.0
    >>> divide(1, 0)
    Traceback (most recent call last):
        ...
    ZeroDivisionError: division by zero
    """
    return a / b


def mean(values):
    """The mean, written down wrong on purpose.

    >>> mean([1, 2, 3, 4])
    2.0
    """
    return sum(values) / len(values)


def later(x):
    """Not ready: every example is skipped.

    >>> later(1)  # doctest: +SKIP
    'soon'
    """
    raise NotImplementedError


def listing(n):
    """A long result, matched with pytest's default ELLIPSIS.

    >>> listing(100)
    [0, 1, 2, ..., 99]
    """
    return list(range(n))


class Counter:
    """Counts up.

    >>> counter = Counter()
    >>> counter.bump()
    1
    >>> counter.bump()
    2
    """

    def __init__(self):
        self.value = 0

    def bump(self):
        """Add one and return the total.

        >>> Counter().bump()
        1
        """
        self.value += 1
        return self.value

    def reset(self):
        """Raises where the example expects nothing.

        >>> Counter().reset()
        """
        raise RuntimeError("reset is not supported")
