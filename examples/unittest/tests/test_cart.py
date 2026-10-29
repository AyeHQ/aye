import io
import os
import unittest
from contextlib import redirect_stdout
from decimal import Decimal
from unittest import mock

from shop import Cart, Inventory, OutOfStock, PaymentDeclined, Product

CATALOG = {
    "apple": Product("apple", "Apple", Decimal("0.50")),
    "bread": Product("bread", "Bread", Decimal("2.40")),
    "coffee": Product("coffee", "Coffee", Decimal("8.99")),
}


class CartTests(unittest.TestCase):
    def setUp(self):
        # No tax, whatever SHOP_TAX_RATE your shell sets. addCleanup restores
        # the environment after the test, pass or fail, even if setUp fails
        # further down.
        environ = mock.patch.dict(os.environ, {"SHOP_TAX_RATE": "0"})
        environ.start()
        self.addCleanup(environ.stop)
        self.cart = Cart(Inventory({"apple": 10, "bread": 3, "coffee": 2}))

    def test_new_cart_is_empty(self):
        self.assertEqual(len(self.cart), 0)
        self.assertEqual(self.cart.total(), Decimal("0.00"))

    def test_add_and_remove(self):
        self.cart.add(CATALOG["apple"], 3)
        self.cart.add(CATALOG["apple"])
        self.cart.add(CATALOG["bread"])
        self.cart.remove("bread")
        self.assertEqual(self.cart.quantity("apple"), 4)
        self.assertEqual(self.cart.quantity("bread"), 0)

    def test_cannot_add_more_than_in_stock(self):
        self.cart.add(CATALOG["coffee"], 2)
        with self.assertRaisesRegex(OutOfStock, "wanted 3, 2 in stock"):
            self.cart.add(CATALOG["coffee"])

    def test_subtotal(self):
        self.cart.add(CATALOG["apple"], 3)
        self.cart.add(CATALOG["coffee"])
        self.assertEqual(self.cart.subtotal(), Decimal("10.49"))

    def test_coupon(self):
        self.cart.add(CATALOG["coffee"], 2)
        self.cart.apply_coupon("WELCOME10")
        self.assertEqual(
            self.cart.summary(),
            {
                "subtotal": Decimal("17.98"),
                "discount": Decimal("1.80"),
                "tax": Decimal("0.00"),
                "total": Decimal("16.18"),
            },
        )

    def test_tax_applies_after_the_discount(self):
        os.environ["SHOP_TAX_RATE"] = "0.20"  # undone by the cleanup from setUp
        self.cart.add(CATALOG["bread"], 2)
        self.cart.apply_coupon("WELCOME10")
        self.assertEqual(self.cart.tax(), Decimal("0.86"))
        self.assertEqual(self.cart.total(), Decimal("5.18"))

    @unittest.expectedFailure
    def test_coupons_stack(self):
        # A cart takes one coupon; stacking isn't built yet.
        self.cart.add(CATALOG["coffee"], 2)
        self.cart.add(CATALOG["bread"], 3)
        self.cart.apply_coupon("WELCOME10")
        self.cart.apply_coupon("FIVEOFF")
        self.assertEqual(self.cart.discount(), Decimal("7.52"))

    def test_print_receipt(self):
        self.cart.add(CATALOG["apple"], 3)
        self.cart.add(CATALOG["coffee"])
        self.cart.apply_coupon("WELCOME10")
        with redirect_stdout(io.StringIO()) as out:
            self.cart.print_receipt()
        self.assertEqual(
            out.getvalue().splitlines(),
            [
                "3 x Apple         1.50",
                "1 x Coffee        8.99",
                "Subtotal         10.49",
                "Discount         -1.05",
                "Tax               0.00",
                "Total             9.44",
            ],
        )

    @unittest.skipIf(
        os.environ.get("AYE_EXAMPLE_FAIL") != "1",
        "fails on purpose: set AYE_EXAMPLE_FAIL=1 to see a failure",
    )
    def test_summary_after_coupon(self):
        # Wrong on purpose: 10% of 10.49 rounds to 1.05, not 1.04.
        self.cart.add(CATALOG["apple"], 3)
        self.cart.add(CATALOG["coffee"])
        self.cart.apply_coupon("WELCOME10")
        self.assertEqual(
            self.cart.summary(),
            {
                "subtotal": Decimal("10.49"),
                "discount": Decimal("1.04"),
                "tax": Decimal("0.00"),
                "total": Decimal("9.45"),
            },
        )


class FakeGateway:
    """A payment gateway that records charges instead of making them."""

    def __init__(self):
        self.charges = []
        self.declining = False

    async def charge(self, amount):
        if self.declining:
            raise PaymentDeclined("card declined")
        self.charges.append(amount)
        return f"pay-{len(self.charges)}"


# As a class decorator, patch.dict applies to each test method in turn.
@mock.patch.dict(os.environ, {"SHOP_TAX_RATE": "0"})
class CheckoutTests(unittest.IsolatedAsyncioTestCase):
    # Test methods can be coroutines; each test runs in a new event loop.

    async def asyncSetUp(self):
        self.inventory = Inventory({"bread": 3, "coffee": 2})
        self.cart = Cart(self.inventory)
        self.gateway = FakeGateway()

    async def test_charges_the_total_and_takes_the_stock(self):
        self.cart.add(CATALOG["bread"], 2)
        order = await self.cart.checkout(self.gateway)
        self.assertEqual(self.gateway.charges, [Decimal("4.80")])
        self.assertEqual(order.items, {"bread": 2})
        self.assertEqual(self.inventory.available("bread"), 1)
        self.assertEqual(len(self.cart), 0)

    async def test_declined_payment_can_be_retried(self):
        self.cart.add(CATALOG["bread"], 2)
        self.gateway.declining = True
        with self.assertRaises(PaymentDeclined):
            await self.cart.checkout(self.gateway)
        self.assertEqual(self.inventory.available("bread"), 3)
        self.gateway.declining = False
        order = await self.cart.checkout(self.gateway)
        self.assertEqual(order.items, {"bread": 2})

    async def test_sold_out_items_are_not_charged(self):
        self.cart.add(CATALOG["coffee"], 2)
        self.inventory.take("coffee", 1)  # someone else bought one meanwhile
        with self.assertRaises(OutOfStock):
            await self.cart.checkout(self.gateway)
        self.assertEqual(self.gateway.charges, [])

    async def test_empty_cart(self):
        with self.assertRaisesRegex(ValueError, "empty"):
            await self.cart.checkout(self.gateway)

    @unittest.skip("gift cards aren't supported yet")
    async def test_gift_card(self):
        self.cart.add(CATALOG["bread"])
        self.cart.apply_gift_card("GIFT-0001")
        await self.cart.checkout(self.gateway)
        self.assertEqual(self.gateway.charges, [])
