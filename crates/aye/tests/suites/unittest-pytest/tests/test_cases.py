"""Only TestCase classes, with inheritance, module fixtures and subtests."""

import unittest

EVENTS = []


def setUpModule():
    EVENTS.append("module")


def tearDownModule():
    EVENTS.clear()


class BaseTests(unittest.TestCase):
    def test_inherited(self):
        self.assertEqual(EVENTS, ["module"])


class ChildTests(BaseTests):
    def test_own(self):
        self.assertTrue(isinstance(self, BaseTests))


class GuardedTests(unittest.TestCase):
    def setUp(self):
        self.assertEqual(EVENTS, [], "setUp fails on an assertion")

    def test_never_reached(self):
        pass


class ParityTests(unittest.TestCase):
    def test_all_odd(self):
        for value in (1, 2, 3):
            with self.subTest(value=value):
                self.assertEqual(value % 2, 1)

    def test_all_positive(self):
        for value in (1, 2, 3):
            with self.subTest(value=value):
                self.assertGreater(value, 0)
