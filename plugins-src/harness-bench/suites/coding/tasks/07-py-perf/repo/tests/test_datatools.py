import unittest

from datatools.contacts import dedupe_contacts
from datatools.grid import count_paths


class ContactsTest(unittest.TestCase):
    def test_merge(self):
        out = dedupe_contacts([
            {"email": "A@x.com", "name": "Ann", "phone": ""},
            {"email": "b@x.com", "name": "Bob"},
            {"email": " a@X.com ", "name": "Annie", "phone": "123"},
        ])
        self.assertEqual(out, [
            {"email": "A@x.com", "name": "Ann", "phone": "123"},
            {"email": "b@x.com", "name": "Bob"},
        ])


class GridTest(unittest.TestCase):
    def test_small(self):
        self.assertEqual(count_paths(["...", "...", "..."]), 6)
        self.assertEqual(count_paths(["..", "#."]), 1)


if __name__ == "__main__":
    unittest.main()
