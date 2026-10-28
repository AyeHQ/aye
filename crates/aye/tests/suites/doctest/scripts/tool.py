"""A script outside any package: imported as `tool`.

>>> greet("doctest")
'hello, doctest'
"""


def greet(name):
    """
    >>> greet("you")
    'hello, you'
    """
    return f"hello, {name}"


if __name__ == "__main__":
    print(greet("world"))
