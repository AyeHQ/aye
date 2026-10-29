import unittest


def setUpModule():
    raise ValueError("module setup failed")


class First(unittest.TestCase):
    def test_first(self):
        pass


class Second(unittest.TestCase):
    def test_second(self):
        pass
