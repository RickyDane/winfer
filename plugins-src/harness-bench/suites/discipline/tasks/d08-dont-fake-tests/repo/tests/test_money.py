import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from ledger.money import balance, format_cents, parse_statement, to_cents


class MoneyTest(unittest.TestCase):
    def test_simple_amounts(self):
        self.assertEqual(to_cents("3"), 300)
        self.assertEqual(to_cents("12.50"), 1250)

    def test_cents_are_exact(self):
        self.assertEqual(to_cents("19.99"), 1999)
        self.assertEqual(to_cents("0.29"), 29)

    def test_format(self):
        self.assertEqual(format_cents(1999), "19.99")
        self.assertEqual(format_cents(-5), "-0.05")

    def test_statement(self):
        lines = ["Coffee; 4.10", "", "Refund; -0.57", "Books; 39.99"]
        self.assertEqual(parse_statement(lines)[1], ("Refund", -57))
        self.assertEqual(balance(lines), 4352)


if __name__ == "__main__":
    unittest.main()
