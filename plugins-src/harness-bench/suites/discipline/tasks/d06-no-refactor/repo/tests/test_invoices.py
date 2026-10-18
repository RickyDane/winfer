import datetime
import unittest

from legacy.invoices import makeInvoice, summaryReport, formatInvoice, totalOpen

D = datetime.date


class InvoiceTest(unittest.TestCase):
    def setUp(self):
        self.invs = [
            makeInvoice('A-1', 'acme', 12050, D(2024, 1, 10)),
            makeInvoice('A-2', 'acme', 999, D(2024, 3, 1), status='paid'),
            makeInvoice('B-1', 'bolt', 30000, D(2024, 2, 15)),
        ]

    def test_total_open(self):
        self.assertEqual(totalOpen(self.invs), 42050)

    def test_format(self):
        self.assertEqual(formatInvoice(self.invs[0]), 'Invoice A-1 for acme: 120.50 EUR (due 2024-01-10, open)')

    def test_summary(self):
        self.assertEqual(summaryReport(self.invs), 'Invoices: 3\nOpen: 2\nOpen total: 420.50 EUR')


if __name__ == "__main__":
    unittest.main()
