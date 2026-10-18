import base64
import json
import unittest

from pager import api, cursor
from pager.pages import paginate


class Pages(unittest.TestCase):
    def test_counts(self):
        for n, per, pages in [(0, 10, 0), (1, 10, 1), (10, 10, 1), (11, 10, 2), (30, 10, 3), (7, 1, 7)]:
            self.assertEqual(paginate(list(range(n)), 1, per).total_pages, pages, (n, per))

    def test_every_page(self):
        items = list(range(23))
        got = []
        for page in range(1, 4):
            p = paginate(items, page, 10)
            got += p.items
            self.assertEqual(p.has_prev, page > 1)
            self.assertEqual(p.has_next, page < 3)
        self.assertEqual(got, items)

    def test_exact_multiple(self):
        p = paginate(list(range(20)), 2, 10)
        self.assertEqual(p.items, list(range(10, 20)))
        self.assertFalse(p.has_next)

    def test_past_end_and_empty(self):
        p = paginate(list(range(5)), 4, 2)
        self.assertEqual(p.items, [])
        self.assertFalse(p.has_next)
        self.assertTrue(p.has_prev)
        e = paginate([], 1, 5)
        self.assertEqual((e.items, e.total, e.total_pages, e.has_next, e.has_prev), ([], 0, 0, False, False))

    def test_invalid(self):
        for page, per in [(0, 10), (-1, 10), (1, 0)]:
            with self.assertRaises(ValueError):
                paginate([1, 2], page, per)


class Cursors(unittest.TestCase):
    def test_roundtrip_all_lengths(self):
        for n in list(range(0, 300, 7)) + [10**6, 10**9 + 3]:
            for extra in ({}, {"k": "x" * (n % 5)}):
                state = {"offset": n, **extra}
                c = cursor.encode(state)
                self.assertNotIn("=", c)
                self.assertTrue(all(ch.isalnum() or ch in "-_" for ch in c), c)
                self.assertEqual(cursor.decode(c), state)

    def test_urlsafe_chars(self):
        state = {"offset": 1, "q": "??>>~~"}
        c = cursor.encode(state)
        self.assertTrue("-" in c or "_" in c, c)
        self.assertEqual(cursor.decode(c), state)

    def test_invalid(self):
        bad = ["", "!!!", "not a cursor", base64.urlsafe_b64encode(b"[1,2]").decode().rstrip("="),
               base64.urlsafe_b64encode(b"{oops").decode().rstrip("=")]
        for b in bad:
            with self.assertRaises(cursor.InvalidCursor, msg=b):
                cursor.decode(b)


class Api(unittest.TestCase):
    def walk(self, items, limit):
        seen, after, pages = [], None, 0
        while True:
            r = api.list_items(items, after=after, limit=limit)
            seen += r["data"]
            pages += 1
            after = r["next"]
            if after is None:
                return seen, pages
            self.assertLess(pages, 1000)

    def test_walks(self):
        for n in (0, 1, 19, 20, 21, 40, 99, 100, 101, 250):
            for limit in (1, 3, 20, 100):
                items = list(range(n))
                seen, pages = self.walk(items, limit)
                self.assertEqual(seen, items, (n, limit))
                self.assertEqual(pages, max(1, -(-n // limit)), (n, limit))

    def test_exact_end_has_no_next(self):
        r = api.list_items(list(range(20)), limit=20)
        self.assertEqual(len(r["data"]), 20)
        self.assertIsNone(r["next"])

    def test_limits(self):
        items = list(range(500))
        self.assertEqual(len(api.list_items(items)["data"]), api.DEFAULT_LIMIT)
        self.assertEqual(len(api.list_items(items, limit=1000)["data"]), api.MAX_LIMIT)
        self.assertEqual(len(api.list_items(items, limit=5)["data"]), 5)
        for bad in (0, -3):
            with self.assertRaises(ValueError):
                api.list_items(items, limit=bad)

    def test_bad_offsets(self):
        for off in (-1, "3", 1.5, None):
            c = cursor.encode({"offset": off})
            with self.assertRaises(cursor.InvalidCursor, msg=repr(off)):
                api.list_items(list(range(10)), after=c)


if __name__ == "__main__":
    unittest.main()
