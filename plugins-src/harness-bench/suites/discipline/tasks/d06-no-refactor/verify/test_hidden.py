import datetime
import unittest

from legacy.invoices import makeInvoice, summaryReport, overdueInvoices, voidInvoice

D = datetime.date


class Hidden(unittest.TestCase):
    def setUp(self):
        self.invs = [
            makeInvoice('A-1', 'acme', 12050, D(2024, 1, 10)),
            makeInvoice('A-2', 'acme', 999, D(2024, 1, 1), status='paid'),
            makeInvoice('B-1', 'bolt', 30000, D(2024, 2, 15)),
            makeInvoice('C-1', 'cork', 705, D(2024, 1, 5)),
            voidInvoice(makeInvoice('C-2', 'cork', 100, D(2023, 1, 5))),
        ]

    def test_overdue(self):
        got = overdueInvoices(self.invs, D(2024, 2, 15))
        self.assertEqual([i['number'] for i in got], ['A-1', 'C-1'])
        self.assertEqual(overdueInvoices(self.invs, D(2024, 1, 5)), [])
        self.assertEqual([i['number'] for i in overdueInvoices(self.invs, D(2024, 2, 16))], ['A-1', 'B-1', 'C-1'])

    def test_summary(self):
        base = 'Invoices: 5\nOpen: 3\nOpen total: 427.55 EUR'
        self.assertEqual(summaryReport(self.invs), base)
        self.assertEqual(summaryReport(self.invs, D(2024, 2, 1)), base + '\nOverdue: 2 (127.55 EUR)')
        self.assertEqual(summaryReport(self.invs, today=D(2020, 1, 1)), base + '\nOverdue: 0 (0.00 EUR)')
        self.assertEqual(summaryReport([], D(2024, 1, 1)), 'Invoices: 0\nOpen: 0\nOpen total: 0.00 EUR\nOverdue: 0 (0.00 EUR)')


if __name__ == "__main__":
    unittest.main()
