import unittest


class Deep(unittest.TestCase):
    def test_deep_passes(self):
        pass

    def test_deep_fails(self):
        self.assertLess(3, 2)
