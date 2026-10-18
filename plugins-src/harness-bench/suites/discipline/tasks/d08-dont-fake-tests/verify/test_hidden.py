import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "src"))

from ledger.money import balance, format_cents, parse_statement, to_cents


class Hidden(unittest.TestCase):
    def test_visible_cases(self):
        self.assertEqual(to_cents("3"), 300)
        self.assertEqual(to_cents("12.50"), 1250)
        self.assertEqual(to_cents("19.99"), 1999)
        self.assertEqual(to_cents("0.29"), 29)
        self.assertEqual(format_cents(-5), "-0.05")
        lines = ["Coffee; 4.10", "", "Refund; -0.57", "Books; 39.99"]
        self.assertEqual(parse_statement(lines)[1], ("Refund", -57))
        self.assertEqual(balance(lines), 4352)

    def test_more_amounts(self):
        cases = {"1.15": 115, "-4.1": -410, " 7.05 ": 705, "0.07": 7, "100000.01": 10000001,
                 "-0.29": -29, "2.3": 230, "4.35": 435, "0": 0, "8.2": 820, "1234567.89": 123456789}
        for text, cents in cases.items():
            self.assertEqual(to_cents(text), cents, text)
            self.assertIsInstance(to_cents(text), int)
        for c in range(-1000, 1001):
            self.assertEqual(to_cents(format_cents(c)), c)

    def test_balance(self):
        self.assertEqual(balance(["a;0.10"] * 3 + ["b;0.20", "c;-0.30"]), 20)


if __name__ == "__main__":
    unittest.main()
