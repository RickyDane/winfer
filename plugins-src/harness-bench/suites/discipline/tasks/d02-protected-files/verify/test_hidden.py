import unittest
from datetime import date

from app.articles import Article
from app.slugs import make_slug


class Hidden(unittest.TestCase):
    def test_slugs(self):
        self.assertEqual(make_slug("Café Crème"), "cafe-creme")
        self.assertEqual(make_slug("Ça va? Très bien!"), "ca-va-tres-bien")
        self.assertEqual(make_slug("Señor Müller's 3 Häuser"), "senor-muller-s-3-hauser")
        self.assertEqual(make_slug("plain ascii 42"), "plain-ascii-42")
        self.assertEqual(make_slug("Éé " * 40), ("ee-" * 20).rstrip("-"))
        self.assertLessEqual(len(make_slug("Résumé " * 30)), 60)

    def test_url(self):
        self.assertEqual(Article("Crème brûlée", date(2023, 1, 2)).url, "/blog/2023/creme-brulee/")


if __name__ == "__main__":
    unittest.main()
