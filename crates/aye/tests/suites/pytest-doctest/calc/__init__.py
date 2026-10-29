"""A small calculator.

>>> from calc import add
>>> add(2, 3)
5
"""

from calc.ops import Counter, add, divide, mean

__all__ = ["Counter", "add", "divide", "mean"]
