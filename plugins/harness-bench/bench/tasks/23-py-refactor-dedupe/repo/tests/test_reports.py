import unittest

from reports import inventory_report, refund_report, sales_report

ROWS = [
    {"region": "north", "amount_cents": 1250, "status": "paid"},
    {"region": "south", "amount_cents": 990, "status": "paid"},
    {"region": "north", "amount_cents": 300, "status": "refunded"},
    {"region": "", "amount_cents": 5000, "status": "paid"},
    {"region": "south", "amount_cents": 120, "status": "pending"},
]


class Reports(unittest.TestCase):
    def test_sales(self):
        self.assertEqual(
            sales_report(ROWS),
            "SALES BY REGION\n"
            "===============\n"
            "(none) (1)                 50.00\n"
            "north (1)                  12.50\n"
            "south (1)                   9.90\n"
            "--------------------------------\n"
            "TOTAL                      72.40\n",
        )

    def test_refunds(self):
        self.assertEqual(
            refund_report(ROWS),
            "REFUNDS BY REGION\n"
            "=================\n"
            "north (1)                 (3.00)\n"
            "--------------------------------\n"
            "TOTAL REFUNDED            (3.00)\n",
        )

    def test_inventory_empty(self):
        self.assertEqual(inventory_report([{"warehouse": "a", "qty": 0}]), "STOCK BY WAREHOUSE\n==================\n(no data)\n")


if __name__ == "__main__":
    unittest.main()
