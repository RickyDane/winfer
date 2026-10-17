import json
import unittest
from datetime import datetime, timezone

from app.models import Address, Customer, LineItem, Order, Status
from app.serde import from_json, to_json


class SerdeTest(unittest.TestCase):
    def test_roundtrip(self):
        o = Order(
            id=7,
            customer=Customer(1, "Ada", Address("1 Main St", "Springfield")),
            created=datetime(2024, 5, 1, 12, 30, tzinfo=timezone.utc),
            status=Status.PAID,
            items=[LineItem("A-1", 2, 9.5)],
        )
        text = to_json(o)
        data = json.loads(text)
        self.assertEqual(data["status"], "paid")
        self.assertEqual(data["created"], "2024-05-01T12:30:00+00:00")
        self.assertEqual(data["items"], [{"sku": "A-1", "qty": 2, "price": 9.5}])
        self.assertEqual(from_json(Order, text), o)

    def test_unknown_key(self):
        with self.assertRaises(ValueError):
            from_json(LineItem, '{"sku": "x", "qty": 1, "price": 1.0, "color": "red"}')


if __name__ == "__main__":
    unittest.main()
