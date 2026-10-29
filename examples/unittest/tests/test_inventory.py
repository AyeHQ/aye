import tempfile
import unittest
from pathlib import Path

from shop import Inventory, OutOfStock


class InventoryTests(unittest.TestCase):
    def setUp(self):
        # Runs before every test: each one gets its own stock to change.
        self.inventory = Inventory({"apple": 10, "bread": 3, "coffee": 2})

    def test_take_reduces_stock(self):
        self.inventory.take("apple", 4)
        self.assertEqual(self.inventory.available("apple"), 6)

    def test_unknown_sku_has_no_stock(self):
        self.assertEqual(self.inventory.available("durian"), 0)

    def test_cannot_take_more_than_available(self):
        with self.assertRaises(OutOfStock) as caught:
            self.inventory.take("coffee", 5)
        self.assertEqual(caught.exception.wanted, 5)
        self.assertEqual(caught.exception.available, 2)
        self.assertEqual(self.inventory.available("coffee"), 2)

    def test_restock_needs_a_positive_quantity(self):
        for quantity in (0, -3):
            with self.subTest(quantity=quantity), self.assertRaises(ValueError):
                self.inventory.restock("apple", quantity)


class InventoryFileTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Runs once for the class: its tests share one temporary directory.
        cls.directory = tempfile.TemporaryDirectory()

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def setUp(self):
        self.path = Path(self.directory.name) / "stock.csv"

    def tearDown(self):
        # Runs after every test, even a failed one: the next test starts
        # without the file.
        self.path.unlink(missing_ok=True)

    def test_save_and_load(self):
        inventory = Inventory({"apple": 10, "durian": 1})
        inventory.save(self.path)
        self.assertTrue(self.path.read_text().startswith("sku,quantity\n"))
        self.assertEqual(Inventory.load(self.path).stock(), inventory.stock())

    def test_load_a_missing_file(self):
        with self.assertRaises(FileNotFoundError):
            Inventory.load(self.path)
