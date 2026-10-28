import unittest


class SharedTests(unittest.TestCase):
    """Imported into test modules: unittest runs it from each one."""

    def test_shared(self):
        self.assertTrue(hasattr(self, "assertEqual"))


class BaseCase(unittest.TestCase):
    """A base with helpers and no tests."""

    def check_positive(self, value):
        self.assertGreater(value, 0)
