import unittest

from pager import api, cursor
from pager.pages import paginate


class PagesTest(unittest.TestCase):
    def test_first_page(self):
        p = paginate(list(range(25)), page=1, per_page=10)
        self.assertEqual(p.items, list(range(10)))
        self.assertEqual(p.total_pages, 3)
        self.assertTrue(p.has_next)
        self.assertFalse(p.has_prev)

    def test_last_page(self):
        p = paginate(list(range(25)), page=3, per_page=10)
        self.assertEqual(p.items, [20, 21, 22, 23, 24])
        self.assertFalse(p.has_next)


class CursorTest(unittest.TestCase):
    def test_roundtrip(self):
        for n in (0, 7, 12345):
            self.assertEqual(cursor.decode(cursor.encode({"offset": n})), {"offset": n})


class ApiTest(unittest.TestCase):
    def test_walk(self):
        items = list(range(45))
        seen, after = [], None
        while True:
            r = api.list_items(items, after=after, limit=20)
            seen += r["data"]
            after = r["next"]
            if after is None:
                break
        self.assertEqual(seen, items)


if __name__ == "__main__":
    unittest.main()
