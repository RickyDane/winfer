import unittest

from textutils import equals_ignore_case, slugify, truncate, word_count


class Basics(unittest.TestCase):
    def test_slugify_ascii(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_truncate_ascii(self):
        self.assertEqual(truncate("abcdef", 4), "abc…")
        self.assertEqual(truncate("abc", 3), "abc")

    def test_compare_ascii(self):
        self.assertTrue(equals_ignore_case("Hello", "hELLO"))

    def test_words_ascii(self):
        self.assertEqual(word_count("it's a test, isn't it"), 5)

    def test_unicode(self):
        self.assertEqual(slugify("Crème Brûlée"), "creme-brulee")


if __name__ == "__main__":
    unittest.main()
