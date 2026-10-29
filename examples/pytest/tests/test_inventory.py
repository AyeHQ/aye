import pytest

from shop import Inventory, OutOfStock


def test_take_reduces_stock(inventory):
    inventory.take("apple", 4)
    assert inventory.available("apple") == 6


def test_unknown_sku_has_no_stock(inventory):
    assert inventory.available("durian") == 0


def test_cannot_take_more_than_available(inventory):
    with pytest.raises(OutOfStock) as excinfo:
        inventory.take("coffee", 5)
    assert excinfo.value.wanted == 5
    assert excinfo.value.available == 2
    assert inventory.available("coffee") == 2


@pytest.mark.parametrize("quantity", [0, -3], ids=["zero", "negative"])
def test_restock_needs_a_positive_quantity(inventory, quantity):
    with pytest.raises(ValueError, match="must be positive"):
        inventory.restock("apple", quantity)


def test_save_and_load(inventory, tmp_path):
    # tmp_path is a new, empty directory for each test.
    path = tmp_path / "stock.csv"
    inventory.restock("durian", 1)
    inventory.save(path)
    assert path.read_text().startswith("sku,quantity\n")
    assert Inventory.load(path).stock() == inventory.stock()


@pytest.mark.slow
def test_many_small_takes(inventory):
    # The `slow` mark is registered in pyproject.toml; `aye -m "not slow"` skips it.
    inventory.restock("apple", 99_990)
    for _ in range(100_000):
        inventory.take("apple", 1)
    assert inventory.available("apple") == 0
