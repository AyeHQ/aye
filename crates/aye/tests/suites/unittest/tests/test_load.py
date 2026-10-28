import unittest


class Parametrized(unittest.TestCase):
    def __init__(self, methodName="runTest", value=0):
        super().__init__(methodName)
        self.value = value

    def test_even(self):
        self.assertEqual(self.value % 2, 0)


def standalone():
    assert True


def load_tests(loader, tests, pattern):
    suite = unittest.TestSuite()
    suite.addTest(Parametrized("test_even", value=4))
    suite.addTest(unittest.FunctionTestCase(standalone))
    return suite
