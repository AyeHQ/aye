"""Coupons: a percentage or a fixed amount off the subtotal."""

from dataclasses import dataclass
from decimal import Decimal

from shop.pricing import money


class UnknownCoupon(LookupError):
    """No coupon has this code."""


@dataclass(frozen=True)
class PercentOff:
    percent: int

    def amount_off(self, subtotal: Decimal) -> Decimal:
        return money(subtotal * self.percent / 100)


@dataclass(frozen=True)
class AmountOff:
    amount: Decimal
    minimum: Decimal = Decimal(0)

    def amount_off(self, subtotal: Decimal) -> Decimal:
        if subtotal < self.minimum:
            return money(0)
        return money(min(self.amount, subtotal))


Discount = PercentOff | AmountOff

_coupons: dict[str, Discount] = {
    "WELCOME10": PercentOff(10),
    "FIVEOFF": AmountOff(Decimal("5.00"), minimum=Decimal("25.00")),
}


def _normalize(code: str) -> str:
    return code.strip().upper()


def coupon(code: str) -> Discount:
    """The discount for a coupon code, ignoring case and surrounding spaces."""
    try:
        return _coupons[_normalize(code)]
    except KeyError:
        raise UnknownCoupon(code) from None


def register_coupon(code: str, discount: Discount) -> None:
    _coupons[_normalize(code)] = discount


def remove_coupon(code: str) -> None:
    del _coupons[_normalize(code)]
