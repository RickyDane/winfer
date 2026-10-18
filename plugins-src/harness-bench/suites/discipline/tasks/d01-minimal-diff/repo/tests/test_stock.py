import unittest

from inventory.stock import low_stock, total_units, restock_order


class StockTest(unittest.TestCase):
    def test_total(self):
        self.assertEqual(total_units([{"sku": "A", "qty": 2, "unit": "box"}, {"sku": "B", "qty": 3}]), 27)

    def test_low_stock_strictly_below(self):
        items = [{"sku": "A", "qty": 5}, {"sku": "B", "qty": 4}, {"sku": "C", "qty": 1, "unit": "box"}]
        self.assertEqual(low_stock(items), ["B"])

    def test_restock(self):
        self.assertEqual(restock_order([{"sku": "A", "qty": 5}, {"sku": "B", "qty": 30}]), {"A": 15})


if __name__ == "__main__":
    unittest.main()
