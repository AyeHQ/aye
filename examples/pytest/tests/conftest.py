"""Fixtures for every test in this directory: pytest finds them by name."""

from decimal import Decimal

import pytest

from shop import (
    Cart,
    Inventory,
    PaymentDeclined,
    PercentOff,
    Product,
    register_coupon,
    remove_coupon,
)


@pytest.fixture(scope="session")
def catalog():
    # Built once for the whole run. Products are frozen, so tests can share them.
    return {
        "apple": Product("apple", "Apple", Decimal("0.50")),
        "bread": Product("bread", "Bread", Decimal("2.40")),
        "coffee": Product("coffee", "Coffee", Decimal("8.99")),
    }


@pytest.fixture(scope="module")
def staff_coupon():
    # Set up once per test module that uses it. The code after `yield` is the
    # teardown: it runs after that module's last test, whether it passed or not.
    register_coupon("STAFF", PercentOff(25))
    yield "STAFF"
    remove_coupon("STAFF")


@pytest.fixture
def inventory():
    # Function scope, the default: each test gets its own stock to change.
    return Inventory({"apple": 10, "bread": 3, "coffee": 2})


@pytest.fixture
def cart(inventory):
    # Fixtures can use fixtures: this cart draws on the test's `inventory`.
    return Cart(inventory)


@pytest.fixture(autouse=True)
def no_tax(monkeypatch):
    # autouse: every test gets it without asking, so a SHOP_TAX_RATE set in
    # your shell can't change the results. monkeypatch undoes it afterwards.
    monkeypatch.delenv("SHOP_TAX_RATE", raising=False)


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


@pytest.fixture
def gateway():
    return FakeGateway()
