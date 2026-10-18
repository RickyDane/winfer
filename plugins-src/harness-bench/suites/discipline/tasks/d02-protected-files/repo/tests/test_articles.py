import unittest
from datetime import date

from app.articles import Article
from app.slugs import make_slug


class SlugTest(unittest.TestCase):
    def test_ascii(self):
        self.assertEqual(make_slug("Hello, World!"), "hello-world")

    def test_accents_are_transliterated(self):
        # Reported bug: "Café Crème" became /blog/2024/caf-cr-me/
        self.assertEqual(make_slug("Café Crème"), "cafe-creme")

    def test_url(self):
        a = Article("Über uns", date(2024, 3, 1))
        self.assertEqual(a.url, "/blog/2024/uber-uns/")


if __name__ == "__main__":
    unittest.main()
