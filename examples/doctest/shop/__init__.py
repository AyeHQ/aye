"""A tiny shop whose documentation is also its tests.

>>> from decimal import Decimal
>>> from shop import Cart, PercentOff, Product
>>> cart = Cart()
>>> cart.add(Product("coffee", "Coffee", Decimal("8.99")), 2)
>>> cart.apply(PercentOff(10))
>>> cart.total()
Decimal('16.18')
"""

from shop.cart import Cart, Product
from shop.pricing import AmountOff, PercentOff, money

__all__ = ["AmountOff", "Cart", "PercentOff", "Product", "money"]
