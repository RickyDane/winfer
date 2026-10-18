import unittest

from shop.cart import Cart
from shop.pricing import apply_discount


class CartTest(unittest.TestCase):
    def test_adding_twice_increases_quantity(self):
        c = Cart()
        c.add("apple", 100, 2)
        c.add("apple", 100, 3)
        self.assertEqual(c.lines[0].qty, 5)

    def test_subtotal_counts_quantity(self):
        c = Cart()
        c.add("pen", 250, 4)
        c.add("pad", 199)
        self.assertEqual(c.subtotal(), 1199)

    def test_discount_rounds_half_up(self):
        self.assertEqual(apply_discount(105, 10), 95)  # 94.5 -> 95

    def test_total(self):
        c = Cart(discount_percent=10)
        c.add("pen", 1000)
        self.assertEqual(c.total(), 1071)


if __name__ == "__main__":
    unittest.main()
