import unittest

EVENTS = []


def setUpModule():
    EVENTS.append("module")


def tearDownModule():
    EVENTS.clear()


class UsesModuleSetup(unittest.TestCase):
    def test_module_ran(self):
        self.assertEqual(EVENTS, ["module"])


class FailingClassSetup(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise RuntimeError("class setup failed")

    def test_blocked_one(self):
        pass

    def test_blocked_two(self):
        pass


class SkippingClassSetup(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise unittest.SkipTest("no network")

    def test_needs_network(self):
        pass


@unittest.skip("whole class")
class SkippedClass(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise RuntimeError("never runs")

    def test_never(self):
        pass


class ClassFixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.resource = "ready"
        cls.addClassCleanup(setattr, cls, "resource", None)

    def test_uses_resource(self):
        self.assertEqual(self.resource, "ready")
