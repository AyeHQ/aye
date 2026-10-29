import unittest
from decimal import Decimal

from shop import AmountOff, PercentOff, UnknownCoupon, coupon, register_coupon, remove_coupon


def setUpModule():
    # Runs once, before the first test in this module.
    register_coupon("STAFF", PercentOff(25))


def tearDownModule():
    # Runs once, after the last test in this module.
    remove_coupon("STAFF")


class CouponTests(unittest.TestCase):
    def test_amount_off(self):
        cases = [
            ("WELCOME10", "20.00", "2.00"),
            ("WELCOME10", "0.05", "0.01"),
            ("FIVEOFF", "30.00", "5.00"),
            ("FIVEOFF", "24.99", "0.00"),
            (" welcome10 ", "20.00", "2.00"),
        ]
        for code, subtotal, expected in cases:
            # Each subTest passes or fails on its own, and a failure names its values.
            with self.subTest(code=code, subtotal=subtotal):
                self.assertEqual(coupon(code).amount_off(Decimal(subtotal)), Decimal(expected))

    def test_fixed_amount_never_exceeds_the_subtotal(self):
        self.assertEqual(AmountOff(Decimal("5.00")).amount_off(Decimal("3.20")), Decimal("3.20"))

    def test_unknown_coupon(self):
        with self.assertRaisesRegex(UnknownCoupon, "SUMMER"):
            coupon("SUMMER")

    def test_staff_coupon(self):
        self.assertEqual(coupon("STAFF"), PercentOff(25))
        self.assertEqual(coupon("staff"), PercentOff(25))
