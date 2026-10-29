"""The shopping cart: items, a coupon, tax, the receipt and checkout."""

from dataclasses import dataclass
from decimal import Decimal
from typing import Protocol

from shop.discounts import Discount, coupon
from shop.inventory import Inventory, OutOfStock
from shop.pricing import money, tax_rate


@dataclass(frozen=True)
class Product:
    sku: str
    name: str
    price: Decimal


@dataclass(frozen=True)
class Order:
    reference: str
    total: Decimal
    items: dict[str, int]


class PaymentDeclined(Exception):
    """The payment gateway refused the charge."""


class Gateway(Protocol):
    async def charge(self, amount: Decimal) -> str:
        """Charge `amount` and return the payment's reference, or raise PaymentDeclined."""


class Cart:
    def __init__(self, inventory: Inventory) -> None:
        self.inventory = inventory
        self._lines: dict[str, tuple[Product, int]] = {}
        self._coupon: Discount | None = None

    def __len__(self) -> int:
        """The number of items in the cart, counting quantities."""
        return sum(quantity for _, quantity in self._lines.values())

    def quantity(self, sku: str) -> int:
        _, quantity = self._lines.get(sku, (None, 0))
        return quantity

    def add(self, product: Product, quantity: int = 1) -> None:
        if quantity < 1:
            raise ValueError(f"quantity must be positive, got {quantity}")
        wanted = self.quantity(product.sku) + quantity
        available = self.inventory.available(product.sku)
        if wanted > available:
            raise OutOfStock(product.sku, wanted, available)
        self._lines[product.sku] = (product, wanted)

    def remove(self, sku: str) -> None:
        del self._lines[sku]

    def apply_coupon(self, code: str) -> None:
        """Use a coupon. A cart takes one; applying another replaces it."""
        self._coupon = coupon(code)

    def subtotal(self) -> Decimal:
        return money(sum((p.price * q for p, q in self._lines.values()), Decimal(0)))

    def discount(self) -> Decimal:
        if self._coupon is None:
            return money(0)
        return self._coupon.amount_off(self.subtotal())

    def tax(self) -> Decimal:
        return money((self.subtotal() - self.discount()) * tax_rate())

    def total(self) -> Decimal:
        return self.subtotal() - self.discount() + self.tax()

    def summary(self) -> dict[str, Decimal]:
        return {
            "subtotal": self.subtotal(),
            "discount": self.discount(),
            "tax": self.tax(),
            "total": self.total(),
        }

    def print_receipt(self) -> None:
        for product, quantity in self._lines.values():
            print(f"{quantity} x {product.name:<10}{money(product.price * quantity):>8}")
        summary = self.summary()
        summary["discount"] = -summary["discount"]
        for label, amount in summary.items():
            print(f"{label.title():<14}{amount:>8}")

    async def checkout(self, gateway: Gateway) -> Order:
        """Charge the total, take the items out of stock and empty the cart.

        Nothing is charged unless every item is still in stock.
        """
        if not self._lines:
            raise ValueError("the cart is empty")
        for product, quantity in self._lines.values():
            available = self.inventory.available(product.sku)
            if quantity > available:
                raise OutOfStock(product.sku, quantity, available)
        total = self.total()
        reference = await gateway.charge(total)
        for product, quantity in self._lines.values():
            self.inventory.take(product.sku, quantity)
        items = {sku: quantity for sku, (_, quantity) in self._lines.items()}
        self._lines.clear()
        self._coupon = None
        return Order(reference, total, items)
