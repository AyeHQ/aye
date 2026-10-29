import unittest


class Ignored(unittest.TestCase):
    """Not a package: discovery never reaches it."""

    def test_ignored(self):
        pass
