"""Stock levels per SKU, saved as CSV."""

import csv
from pathlib import Path


class OutOfStock(Exception):
    """Fewer items are in stock than were asked for."""

    def __init__(self, sku: str, wanted: int, available: int) -> None:
        super().__init__(f"{sku}: wanted {wanted}, {available} in stock")
        self.sku = sku
        self.wanted = wanted
        self.available = available


class Inventory:
    def __init__(self, stock: dict[str, int] | None = None) -> None:
        self._stock = dict(stock or {})

    def available(self, sku: str) -> int:
        return self._stock.get(sku, 0)

    def stock(self) -> dict[str, int]:
        """Every SKU's quantity, as a new dict."""
        return dict(self._stock)

    def restock(self, sku: str, quantity: int) -> None:
        if quantity < 1:
            raise ValueError(f"quantity must be positive, got {quantity}")
        self._stock[sku] = self.available(sku) + quantity

    def take(self, sku: str, quantity: int) -> None:
        available = self.available(sku)
        if quantity > available:
            raise OutOfStock(sku, quantity, available)
        self._stock[sku] = available - quantity

    def save(self, path: Path) -> None:
        with path.open("w", newline="") as file:
            writer = csv.writer(file)
            writer.writerow(["sku", "quantity"])
            writer.writerows(sorted(self._stock.items()))

    @classmethod
    def load(cls, path: Path) -> "Inventory":
        with path.open(newline="") as file:
            return cls({row["sku"]: int(row["quantity"]) for row in csv.DictReader(file)})
