import unittest

from inventory.stock import low_stock, to_units, by_location


class Hidden(unittest.TestCase):
    def test_low_stock(self):
        items = [
            {"sku": "A", "qty": 5},
            {"sku": "B", "qty": 4},
            {"sku": "C", "qty": 1, "unit": "box"},
            {"sku": "D", "qty": 0},
        ]
        self.assertEqual(low_stock(items), ["B", "D"])
        self.assertEqual(low_stock(items, threshold=12), ["A", "B", "D"])
        self.assertEqual(low_stock(items, threshold=13), ["A", "B", "C", "D"])
        self.assertEqual(low_stock([], 3), [])

    def test_unchanged(self):
        self.assertEqual(to_units(2, "pallet"), 960)
        self.assertEqual(by_location([{"sku": "A", "qty": 1}]), {"unknown": ["A"]})


if __name__ == "__main__":
    unittest.main()
