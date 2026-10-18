import unicodedata
import unittest

from textutils import equals_ignore_case, slugify, truncate, word_count

nfd = lambda s: unicodedata.normalize("NFD", s)


class Slugify(unittest.TestCase):
    def test_examples(self):
        cases = {
            "Crème Brûlée": "creme-brulee",
            nfd("Crème Brûlée"): "creme-brulee",
            "Straße 5": "strasse-5",
            "GROẞE STRAẞE": "grosse-strasse",
            "Ærøskøbing": "aeroskobing",
            "Œuvre Łódź Đakovo Þór": "oeuvre-lodz-dakovo-thor",
            "日本語": "n-a",
            "Hello, 🌍 World!": "hello-world",
            "ﬁnal ① ²": "final-1-2",
            "--Ünïcödé--": "unicode",
            "   ": "n-a",
            "Ελληνικά and ASCII": "and-ascii",
            "São Paulo — 2026": "sao-paulo-2026",
        }
        for src, want in cases.items():
            self.assertEqual(slugify(src), want, repr(src))


class Truncate(unittest.TestCase):
    def test_unchanged_when_short(self):
        for s, n in [("été", 3), ("👩‍💻👩‍💻", 2), ("🇩🇪🇫🇷", 2), ("", 1), ("👍🏽x", 2)]:
            self.assertEqual(truncate(s, n), s, repr(s))

    def test_cuts_whole_characters(self):
        self.assertEqual(truncate("été sun", 3), "ét…")
        self.assertEqual(truncate("👩‍💻👨‍👩‍👧abc", 3), "👩‍💻👨‍👩‍👧…")
        self.assertEqual(truncate("🇩🇪🇫🇷🇮🇹", 2), "🇩🇪…")
        self.assertEqual(truncate("🇩🇪🇫🇷🇮🇹", 3), "🇩🇪🇫🇷🇮🇹")
        self.assertEqual(truncate("👍🏽👍🏽👍🏽", 2), "👍🏽…")
        self.assertEqual(truncate("❤️❤️❤️", 2), "❤️…")
        self.assertEqual(truncate("ä́bcd", 2), "ä́…")
        self.assertEqual(truncate("Straße", 4), "Str…")
        self.assertEqual(truncate("abc", 1), "…")
        self.assertEqual(truncate("ab", 1), "…")

    def test_bad_limit(self):
        with self.assertRaises(ValueError):
            truncate("x", 0)


class Compare(unittest.TestCase):
    def test_equal(self):
        for a, b in [("Straße", "STRASSE"), ("é", "é"), ("É", "é"), ("ΣΊΣΥΦΟΣ", "σίσυφος"),
                     ("ǅungla", "ǄUNGLA"), ("ﬀ", "FF"), ("Ⅻ", "ⅻ"), ("Ångström", "ångström")]:
            self.assertTrue(equals_ignore_case(a, b), (a, b))

    def test_not_equal(self):
        for a, b in [("é", "e"), ("Straße", "Strase"), ("a", "á"), ("ss", "s")]:
            self.assertFalse(equals_ignore_case(a, b), (a, b))


class Words(unittest.TestCase):
    def test_counts(self):
        cases = {
            "Ça va? Très bien — naïve café-crème": 7,
            nfd("Ça va? Très bien — naïve café-crème"): 7,
            "don’t stop, it's 'quoted' text": 5,
            "Привет, мир! 你好": 3,
            "🎉🎉 party 🎉": 1,
            "x - y — z": 3,
            "rock'n'roll l’été": 2,
            "  ": 0,
            "'": 0,
            "über1 2nd 3": 3,
            "é'té": 1,
        }
        for src, want in cases.items():
            self.assertEqual(word_count(src), want, repr(src))


if __name__ == "__main__":
    unittest.main()
