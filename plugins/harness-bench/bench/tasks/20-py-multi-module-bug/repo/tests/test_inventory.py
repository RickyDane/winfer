import unittest

from inventory import Store, available, cancel, fulfill, reserve, restock


class InventoryTest(unittest.TestCase):
    def setUp(self):
        self.s = Store()
        restock(self.s, "apple", 10)

    def test_reserve_and_fulfill(self):
        r = reserve(self.s, "apple", 4)
        self.assertEqual(available(self.s, "apple"), 6)
        fulfill(self.s, r.id)
        self.assertEqual(self.s.item("apple").on_hand, 6)
        self.assertEqual(available(self.s, "apple"), 6)

    def test_cannot_over_reserve(self):
        reserve(self.s, "apple", 8)
        with self.assertRaises(ValueError):
            reserve(self.s, "apple", 3)

    def test_cancel_releases(self):
        r = reserve(self.s, "apple", 5)
        cancel(self.s, r.id)
        self.assertEqual(available(self.s, "apple"), 10)
        with self.assertRaises(ValueError):
            cancel(self.s, r.id)

    def test_cancel_then_restock(self):
        r = reserve(self.s, "apple", 5)
        cancel(self.s, r.id)
        restock(self.s, "apple", 3)
        self.assertEqual(available(self.s, "apple"), 13)


if __name__ == "__main__":
    unittest.main()
