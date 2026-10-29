import sys
import unittest


class Outcomes(unittest.TestCase):
    def test_passes(self):
        self.assertEqual(1 + 1, 2)

    def test_fails_with_values(self):
        self.assertEqual([1, 2, 3], [1, 2, 4])

    def test_raises(self):
        {}["missing"]

    @unittest.skip("not today")
    def test_skipped(self):
        pass

    def test_skips_inside(self):
        self.skipTest("decided at runtime")

    @unittest.expectedFailure
    def test_expected_failure(self):
        self.assertTrue(False)

    @unittest.expectedFailure
    def test_unexpected_success(self):
        pass

    def test_prints_and_fails(self):
        print("captured output")
        print("captured error", file=sys.stderr)
        self.assertIn("x", "abc")


class Phases(unittest.TestCase):
    def setUp(self):
        if self._testMethodName == "test_setup_error":
            raise RuntimeError("setUp failed")

    def tearDown(self):
        if self._testMethodName == "test_teardown_error":
            raise RuntimeError("tearDown failed")

    def test_setup_error(self):
        pass

    def test_teardown_error(self):
        pass

    def test_cleanup_error(self):
        self.addCleanup(lambda: 1 / 0)


class Subtests(unittest.TestCase):
    def test_all_pass(self):
        for i in range(3):
            with self.subTest(i=i):
                self.assertGreaterEqual(i, 0)

    def test_one_fails(self):
        for i in range(3):
            with self.subTest(i=i):
                self.assertNotEqual(i, 1)

    def test_one_errors(self):
        with self.subTest("boom"):
            raise ValueError("bad value")

    def test_one_skips(self):
        with self.subTest("later"):
            self.skipTest("not yet")


if __name__ == "__main__":
    unittest.main()
