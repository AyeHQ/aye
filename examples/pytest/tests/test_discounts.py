from decimal import Decimal

import pytest

from shop import AmountOff, PercentOff, UnknownCoupon, coupon


# pytest.param(..., id=...) names each case: test_amount_off[below-minimum].
@pytest.mark.parametrize(
    ("code", "subtotal", "expected"),
    [
        pytest.param("WELCOME10", "20.00", "2.00", id="percent"),
        pytest.param("WELCOME10", "0.05", "0.01", id="percent-rounds-half-up"),
        pytest.param("FIVEOFF", "30.00", "5.00", id="fixed-amount"),
        pytest.param("FIVEOFF", "24.99", "0.00", id="below-minimum"),
        pytest.param(" welcome10 ", "20.00", "2.00", id="code-ignores-case-and-spaces"),
    ],
)
def test_amount_off(code, subtotal, expected):
    assert coupon(code).amount_off(Decimal(subtotal)) == Decimal(expected)


def test_fixed_amount_never_exceeds_the_subtotal():
    assert AmountOff(Decimal("5.00")).amount_off(Decimal("3.20")) == Decimal("3.20")


def test_unknown_coupon():
    with pytest.raises(UnknownCoupon, match="SUMMER"):
        coupon("SUMMER")


def test_staff_coupon(staff_coupon):
    assert coupon(staff_coupon) == PercentOff(25)


def test_staff_coupon_ignores_case(staff_coupon):
    # Same module, so this reuses the registration made for the test above.
    assert coupon(staff_coupon.lower()) == PercentOff(25)
