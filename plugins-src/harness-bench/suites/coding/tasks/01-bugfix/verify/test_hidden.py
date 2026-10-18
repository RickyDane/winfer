import unittest

from shop.cart import Cart
from shop.pricing import apply_discount, with_tax


class Hidden(unittest.TestCase):
    def test_discount_rounding(self):
        self.assertEqual(apply_discount(15, 10), 14)  # 13.5 -> 14
        self.assertEqual(apply_discount(999, 33), 669)  # 669.33 -> 669
        self.assertEqual(apply_discount(1000, 0), 1000)
        self.assertEqual(apply_discount(1000, 100), 0)

    def test_quantities_and_totals(self):
        c = Cart(discount_percent=25)
        c.add("a", 333, 3)
        c.add("b", 101)
        c.add("a", 333, 1)
        self.assertEqual(c.subtotal(), 333 * 4 + 101)
        self.assertEqual(c.total(), with_tax(apply_discount(1433, 25)))

    def test_bad_qty(self):
        with self.assertRaises(ValueError):
            Cart().add("x", 1, 0)


if __name__ == "__main__":
    unittest.main()
