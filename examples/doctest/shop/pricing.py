"""Money and coupons.

Amounts are decimals rounded to the cent, so a price never shows a fraction
of a cent:

>>> money("2.675")
Decimal('2.68')
"""

from dataclasses import dataclass
from decimal import ROUND_HALF_UP, Decimal

CENT = Decimal("0.01")


def money(value: Decimal | int | str) -> Decimal:
    """Round to the cent, halves away from zero.

    >>> money(3)
    Decimal('3.00')
    >>> money("0.125"), money("-0.125")
    (Decimal('0.13'), Decimal('-0.13'))

    A float can't hold most cents exactly, so it is refused. For an exception,
    doctest checks only the first line and the last; the stack in between is
    written as `...`:

    >>> money(0.1)
    Traceback (most recent call last):
        ...
    TypeError: use a str, int or Decimal, not a float
    """
    if isinstance(value, float):
        raise TypeError("use a str, int or Decimal, not a float")
    return Decimal(value).quantize(CENT, rounding=ROUND_HALF_UP)


@dataclass(frozen=True)
class PercentOff:
    """A coupon worth a percentage of the subtotal.

    >>> welcome = PercentOff(10)
    >>> welcome
    PercentOff(percent=10)
    >>> welcome.amount_off(Decimal("20.00"))
    Decimal('2.00')
    """

    percent: int

    def amount_off(self, subtotal: Decimal) -> Decimal:
        """The discount on `subtotal`, rounded to the cent.

        >>> PercentOff(10).amount_off(Decimal("10.49"))
        Decimal('1.05')
        """
        return money(subtotal * self.percent / 100)


@dataclass(frozen=True)
class AmountOff:
    """A coupon worth a fixed amount, once the subtotal reaches a minimum.

    >>> five_off = AmountOff(Decimal("5.00"), minimum=Decimal("25.00"))
    >>> five_off.amount_off(Decimal("30.00")), five_off.amount_off(Decimal("24.99"))
    (Decimal('5.00'), Decimal('0.00'))
    """

    amount: Decimal
    minimum: Decimal = Decimal(0)

    def amount_off(self, subtotal: Decimal) -> Decimal:
        """The discount on `subtotal`: never more than the subtotal itself.

        >>> AmountOff(Decimal("5.00")).amount_off(Decimal("3.20"))
        Decimal('3.20')
        """
        if subtotal < self.minimum:
            return money(0)
        return money(min(self.amount, subtotal))
