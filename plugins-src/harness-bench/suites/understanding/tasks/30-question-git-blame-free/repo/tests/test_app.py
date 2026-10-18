import unittest

from app.main import build


class App(unittest.TestCase):
    def test_build(self):
        app = build()
        self.assertEqual(app["http"].get("users").status, 200)
        self.assertEqual(app["billing"].push_invoice({"id": 1}), 200)
        self.assertEqual(app["events"].published, [("invoice.synced", {"id": 1})])


if __name__ == "__main__":
    unittest.main()
