"""Shapes and their areas.

>>> Square(2).area() + Circle(1).area()
7.14
"""

import math


class Shape:
    """Something with an area.

    >>> Shape().area()
    Traceback (most recent call last):
    ...
    NotImplementedError: Shape.area
    """

    def area(self):
        raise NotImplementedError(f"{type(self).__name__}.area")

    @property
    def name(self):
        """The shape's name.

        >>> Square(1).name
        'square'
        """
        return type(self).__name__.lower()

    @staticmethod
    def unit():
        """
        >>> Shape.unit()
        1
        """
        return 1

    @classmethod
    def build(cls, *args):
        """
        >>> Square.build(2).side
        2
        """
        return cls(*args)

    class Registry:
        """
        >>> Shape.Registry().names()
        []
        """

        def names(self):
            """
            >>> Shape.Registry().names() == []
            True
            """
            return []


class Square(Shape):
    def __init__(self, side):
        self.side = side

    def area(self):
        """
        >>> Square(3).area()
        9
        >>> Square(4).area()
        15
        >>> Square(5).area()
        25
        """
        return self.side**2


class Circle(Shape):
    def __init__(self, radius):
        self.radius = radius

    def area(self):
        """
        >>> Circle(1).area()
        3.14
        >>> Circle(0).area()
        0.0
        """
        return math.pi * self.radius**2


def describe(shape):
    """
    >>> describe(Square(2))
    'a square   of area 4'
    >>> describe(Square(2))  # doctest: +NORMALIZE_WHITESPACE
    'a square of area 4'
    """
    return f"a {shape.name} of area {shape.area()}"


def explode():
    """
    >>> explode()
    'never'
    """
    return {}["missing"]


def not_yet():
    """
    >>> not_yet()  # doctest: +SKIP
    'someday'
    >>> 1 + 1  # doctest: +SKIP
    3
    """


def partly():
    """
    >>> partly()  # doctest: +SKIP
    'skipped'
    >>> list(range(10))
    [0, 1, ..., 9]
    """
    return "done"
