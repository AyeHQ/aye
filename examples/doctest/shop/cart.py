"""A shopping cart.

Each docstring's examples run with the module's globals, so `Cart`,
`Product` and `Decimal` need no import here:

>>> cart = Cart()
>>> cart.add(Product("apple", "Apple", Decimal("0.50")), 3)
>>> cart.total()
Decimal('1.50')
"""

from dataclasses import dataclass
from decimal import Decimal

from shop.pricing import AmountOff, PercentOff, money


@dataclass(frozen=True)
class Product:
    """Something to sell.

    >>> Product("apple", "Apple", Decimal("0.50"))
    Product(sku='apple', name='Apple', price=Decimal('0.50'))
    """

    sku: str
    name: str
    price: Decimal


class Cart:
    """Products and their quantities, with at most one coupon.

    >>> cart = Cart()
    >>> len(cart), cart.total()
    (0, Decimal('0.00'))
    """

    def __init__(self) -> None:
        self._lines: dict[str, tuple[Product, int]] = {}
        self._coupon: PercentOff | AmountOff | None = None

    def __len__(self) -> int:
        return sum(quantity for _, quantity in self._lines.values())

    def add(self, product: Product, quantity: int = 1) -> None:
        """Put `quantity` more of `product` in the cart.

        >>> cart = Cart()
        >>> apple = Product("apple", "Apple", Decimal("0.50"))
        >>> cart.add(apple, 2)
        >>> cart.add(apple)
        >>> len(cart)
        3
        >>> cart.add(apple, 0)
        Traceback (most recent call last):
            ...
        ValueError: quantity must be positive, got 0
        """
        if quantity < 1:
            raise ValueError(f"quantity must be positive, got {quantity}")
        _, in_cart = self._lines.get(product.sku, (product, 0))
        self._lines[product.sku] = (product, in_cart + quantity)

    def apply(self, coupon: PercentOff | AmountOff) -> None:
        """Use a coupon. A cart takes one; applying another replaces it."""
        self._coupon = coupon

    def subtotal(self) -> Decimal:
        return money(sum((p.price * q for p, q in self._lines.values()), Decimal(0)))

    def discount(self) -> Decimal:
        if self._coupon is None:
            return money(0)
        return self._coupon.amount_off(self.subtotal())

    def total(self) -> Decimal:
        return self.subtotal() - self.discount()

    def summary(self) -> dict[str, Decimal]:
        """The amounts, by name.

        `+ELLIPSIS` lets `...` in the expected output stand for anything:

        >>> cart = Cart()
        >>> cart.add(Product("coffee", "Coffee", Decimal("8.99")), 2)
        >>> cart.apply(PercentOff(10))
        >>> cart.summary()  # doctest: +ELLIPSIS
        {'subtotal': Decimal('17.98'), ..., 'total': Decimal('16.18')}
        """
        return {"subtotal": self.subtotal(), "discount": self.discount(), "total": self.total()}

    def print_receipt(self) -> None:
        """Print a line per product, then the totals.

        >>> cart = Cart()
        >>> cart.add(Product("apple", "Apple", Decimal("0.50")), 3)
        >>> cart.apply(PercentOff(10))
        >>> cart.print_receipt()
        3 x Apple         1.50
        Subtotal          1.50
        Discount         -0.15
        Total             1.35
        """
        for product, quantity in self._lines.values():
            print(f"{quantity} x {product.name:<10}{money(product.price * quantity):>8}")
        summary = self.summary()
        summary["discount"] = -summary["discount"]
        for label, amount in summary.items():
            print(f"{label.title():<14}{amount:>8}")
