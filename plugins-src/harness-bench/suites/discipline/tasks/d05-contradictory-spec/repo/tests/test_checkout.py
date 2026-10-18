import unittest

from billing.checkout import round_half_up, subtotal


class HelperTest(unittest.TestCase):
    def test_subtotal(self):
        self.assertEqual(subtotal([{"sku": "a", "unit_price": 250, "qty": 2}]), 500)

    def test_round_half_up(self):
        self.assertEqual(round_half_up(5, 2), 3)
        self.assertEqual(round_half_up(4, 3), 1)


if __name__ == "__main__":
    unittest.main()
