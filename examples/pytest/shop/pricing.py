"""Money and sales tax."""

import os
from decimal import ROUND_HALF_UP, Decimal

CENT = Decimal("0.01")


def money(value: Decimal | int | str) -> Decimal:
    """Round to the cent, halves away from zero."""
    return Decimal(value).quantize(CENT, rounding=ROUND_HALF_UP)


def tax_rate() -> Decimal:
    """The sales tax rate, from `SHOP_TAX_RATE` (`"0.20"` is 20%). None by default."""
    return Decimal(os.environ.get("SHOP_TAX_RATE", "0"))
