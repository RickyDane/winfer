import unittest

from inventory import Store, adjust, available, cancel, fulfill, reserve, restock
from inventory.models import CANCELLED
from inventory.report import summary


class Hidden(unittest.TestCase):
    def setUp(self):
        self.s = Store()
        restock(self.s, "apple", 10)
        restock(self.s, "pear", 4)

    def test_status_is_cancelled_constant(self):
        r = reserve(self.s, "apple", 2)
        cancel(self.s, r.id)
        self.assertEqual(self.s.reservations[r.id].status, CANCELLED)
        self.assertEqual(r.history, ["reserved", "cancelled"])

    def test_cancel_then_adjust(self):
        r = reserve(self.s, "apple", 6)
        cancel(self.s, r.id)
        self.assertEqual(adjust(self.s, "apple", 9), 9)

    def test_cancel_then_dump_load(self):
        a = reserve(self.s, "apple", 3)
        b = reserve(self.s, "apple", 4)
        cancel(self.s, a.id)
        s2 = Store.load(self.s.dump())
        self.assertEqual(available(s2, "apple"), 6)
        self.assertEqual(s2.item("apple").reserved, 4)
        fulfill(s2, b.id)
        self.assertEqual(available(s2, "apple"), 6)
        self.assertEqual(reserve(s2, "apple", 6).id, 3)

    def test_summary_counts(self):
        a = reserve(self.s, "apple", 1)
        b = reserve(self.s, "apple", 2)
        c = reserve(self.s, "pear", 3)
        cancel(self.s, a.id)
        fulfill(self.s, c.id)
        rep = summary(self.s)
        self.assertEqual(rep["_status"], {"active": 1, "cancelled": 1, "fulfilled": 1})
        self.assertEqual(rep["apple"], {"on_hand": 10, "reserved": 2, "available": 8, "open": 1})
        self.assertEqual(rep["pear"], {"on_hand": 1, "reserved": 0, "available": 1, "open": 0})

    def test_long_sequence(self):
        rs = [reserve(self.s, "apple", 2) for _ in range(5)]
        with self.assertRaises(ValueError):
            reserve(self.s, "apple", 1)
        cancel(self.s, rs[0].id)
        restock(self.s, "apple", 1)
        cancel(self.s, rs[1].id)
        fulfill(self.s, rs[2].id)
        restock(self.s, "apple", 2)
        self.assertEqual(self.s.item("apple").on_hand, 11)
        self.assertEqual(available(self.s, "apple"), 7)
        reserve(self.s, "apple", 7)
        with self.assertRaises(ValueError):
            reserve(self.s, "apple", 1)

    def test_cancel_fulfilled_rejected(self):
        r = reserve(self.s, "pear", 1)
        fulfill(self.s, r.id)
        with self.assertRaises(ValueError):
            cancel(self.s, r.id)
        restock(self.s, "pear", 1)
        self.assertEqual(available(self.s, "pear"), 4)


if __name__ == "__main__":
    unittest.main()
