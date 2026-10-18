import unittest
from decimal import Decimal

from billing.invoice import invoice_total
from billing.models import Line, Order


def order(region, code=None):
    return Order("c1", region, [Line("A", 2, "10.00"), Line("B", 1, "5.00")], code)


class InvoiceTest(unittest.TestCase):
    def test_eu_no_discount(self):
        self.assertEqual(invoice_total(order("DE")), Decimal("29.75"))

    def test_us_discount(self):
        self.assertEqual(invoice_total(order("US", "SPRING10")), Decimal("22.50"))

    def test_eu_discount(self):
        # 25.00 net -> 29.75 gross -> 10% off -> 26.78 (rounded per line)
        self.assertEqual(invoice_total(order("DE", "SPRING10")), Decimal("26.78"))


if __name__ == "__main__":
    unittest.main()
