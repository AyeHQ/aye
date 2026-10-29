import asyncio
import os
from decimal import Decimal

import pytest

from shop import OutOfStock, PaymentDeclined


def test_new_cart_is_empty(cart):
    assert len(cart) == 0
    assert cart.total() == Decimal("0.00")


def test_add_and_remove(cart, catalog):
    cart.add(catalog["apple"], 3)
    cart.add(catalog["apple"])
    cart.add(catalog["bread"])
    cart.remove("bread")
    assert cart.quantity("apple") == 4
    assert cart.quantity("bread") == 0


def test_cannot_add_more_than_in_stock(cart, catalog):
    cart.add(catalog["coffee"], 2)
    with pytest.raises(OutOfStock, match="wanted 3, 2 in stock"):
        cart.add(catalog["coffee"])


class TestTotals:
    # A class groups related tests. Each method still gets fresh fixtures.

    def test_subtotal(self, cart, catalog):
        cart.add(catalog["apple"], 3)
        cart.add(catalog["coffee"])
        assert cart.subtotal() == Decimal("10.49")

    def test_coupon(self, cart, catalog):
        cart.add(catalog["coffee"], 2)
        cart.apply_coupon("WELCOME10")
        assert cart.summary() == {
            "subtotal": Decimal("17.98"),
            "discount": Decimal("1.80"),
            "tax": Decimal("0.00"),
            "total": Decimal("16.18"),
        }

    def test_tax_applies_after_the_discount(self, cart, catalog, monkeypatch):
        monkeypatch.setenv("SHOP_TAX_RATE", "0.20")
        cart.add(catalog["bread"], 2)
        cart.apply_coupon("WELCOME10")
        assert (cart.tax(), cart.total()) == (Decimal("0.86"), Decimal("5.18"))

    @pytest.mark.xfail(reason="a cart takes one coupon; stacking isn't built yet", strict=True)
    def test_coupons_stack(self, cart, catalog):
        cart.add(catalog["coffee"], 2)
        cart.add(catalog["bread"], 3)
        cart.apply_coupon("WELCOME10")
        cart.apply_coupon("FIVEOFF")
        assert cart.discount() == Decimal("7.52")


def test_print_receipt(cart, catalog, capsys):
    cart.add(catalog["apple"], 3)
    cart.add(catalog["coffee"])
    cart.apply_coupon("WELCOME10")
    cart.print_receipt()
    assert capsys.readouterr().out.splitlines() == [
        "3 x Apple         1.50",
        "1 x Coffee        8.99",
        "Subtotal         10.49",
        "Discount         -1.05",
        "Tax               0.00",
        "Total             9.44",
    ]


@pytest.mark.skipif(
    os.environ.get("AYE_EXAMPLE_FAIL") != "1",
    reason="fails on purpose: set AYE_EXAMPLE_FAIL=1 to see a failure",
)
def test_summary_after_coupon(cart, catalog):
    # Wrong on purpose: 10% of 10.49 rounds to 1.05, not 1.04. The report
    # shows only the dict entries that differ.
    cart.add(catalog["apple"], 3)
    cart.add(catalog["coffee"])
    cart.apply_coupon("WELCOME10")
    assert cart.summary() == {
        "subtotal": Decimal("10.49"),
        "discount": Decimal("1.04"),
        "tax": Decimal("0.00"),
        "total": Decimal("9.45"),
    }


class TestCheckout:
    # Plain pytest has no async tests, so these drive the coroutine with asyncio.run.

    def test_charges_the_total_and_takes_the_stock(self, cart, catalog, inventory, gateway):
        cart.add(catalog["bread"], 2)
        order = asyncio.run(cart.checkout(gateway))
        assert gateway.charges == [Decimal("4.80")]
        assert order.items == {"bread": 2}
        assert inventory.available("bread") == 1
        assert len(cart) == 0

    def test_declined_payment_can_be_retried(self, cart, catalog, inventory, gateway):
        cart.add(catalog["bread"], 2)
        gateway.declining = True
        with pytest.raises(PaymentDeclined):
            asyncio.run(cart.checkout(gateway))
        assert inventory.available("bread") == 3
        gateway.declining = False
        assert asyncio.run(cart.checkout(gateway)).items == {"bread": 2}

    def test_sold_out_items_are_not_charged(self, cart, catalog, inventory, gateway):
        cart.add(catalog["coffee"], 2)
        inventory.take("coffee", 1)  # someone else bought one meanwhile
        with pytest.raises(OutOfStock):
            asyncio.run(cart.checkout(gateway))
        assert gateway.charges == []

    def test_empty_cart(self, cart, gateway):
        with pytest.raises(ValueError, match="empty"):
            asyncio.run(cart.checkout(gateway))

    @pytest.mark.skip(reason="gift cards aren't supported yet")
    def test_gift_card(self, cart, catalog, gateway):
        cart.add(catalog["bread"])
        cart.apply_gift_card("GIFT-0001")
        asyncio.run(cart.checkout(gateway))
        assert gateway.charges == []
