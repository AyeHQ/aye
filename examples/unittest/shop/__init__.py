"""A tiny shop: products, a cart, coupons and stock."""

from shop.cart import Cart, Order, PaymentDeclined, Product
from shop.discounts import (
    AmountOff,
    PercentOff,
    UnknownCoupon,
    coupon,
    register_coupon,
    remove_coupon,
)
from shop.inventory import Inventory, OutOfStock
from shop.pricing import money, tax_rate

__all__ = [
    "AmountOff",
    "Cart",
    "Inventory",
    "Order",
    "OutOfStock",
    "PaymentDeclined",
    "PercentOff",
    "Product",
    "UnknownCoupon",
    "coupon",
    "money",
    "register_coupon",
    "remove_coupon",
    "tax_rate",
]
