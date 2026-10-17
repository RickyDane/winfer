from __future__ import annotations

import enum
import json
import unittest
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from typing import Optional

from app.models import Address, Customer, LineItem, Order, Priority, Status
from app.serde import from_json, to_json


class Color(enum.Enum):
    RED = "r"
    GREEN = "g"


class Level(enum.Enum):
    ONE = 1
    TWO = 2


@dataclass
class Leaf:
    n: int
    when: Optional[datetime] = None


@dataclass
class Node:
    name: str
    leaves: list[Leaf]
    child: Optional[Node] = None
    colors: list[Color] = field(default_factory=list)
    scores: list[Optional[float]] = field(default_factory=list)
    maybe: Optional[list[int]] = None
    level: Level = Level.ONE
    ratio: float = 1.0


def order():
    return Order(
        id=42,
        customer=Customer(3, "Grace", Address("2 Elm", "Paris", "75001"), vip=True),
        created=datetime(2024, 1, 2, 3, 4, 5, 678901, tzinfo=timezone(timedelta(hours=-5))),
        status=Status.SHIPPED,
        items=[LineItem("A", 1, 2.5), LineItem("B", 3, 10.0)],
        shipped=datetime(2024, 1, 3, 8, 0),
        priority=Priority.HIGH,
        tags=["gift", "fragile"],
        note="leave at door",
    )


class Hidden(unittest.TestCase):
    def assertBad(self, cls, data, word=None):
        text = data if isinstance(data, str) else json.dumps(data)
        with self.assertRaises(ValueError) as cm:
            from_json(cls, text)
        if word:
            self.assertIn(word, str(cm.exception))

    def test_full_order_roundtrip(self):
        o = order()
        data = json.loads(to_json(o))
        self.assertEqual(data["priority"], 2)
        self.assertEqual(data["shipped"], "2024-01-03T08:00:00")
        self.assertEqual(data["created"], "2024-01-02T03:04:05.678901-05:00")
        self.assertEqual(data["customer"]["address"], {"street": "2 Elm", "city": "Paris", "zip": "75001"})
        self.assertIs(data["customer"]["vip"], True)
        back = from_json(Order, to_json(o))
        self.assertEqual(back, o)
        self.assertIs(back.status, Status.SHIPPED)
        self.assertIs(back.priority, Priority.HIGH)
        self.assertIsInstance(back.items[0], LineItem)
        self.assertIsInstance(back.customer.address, Address)

    def test_defaults_when_missing(self):
        text = json.dumps({"id": 1, "customer": {"id": 2, "name": "x", "address": {"street": "s", "city": "c"}},
                           "created": "2024-01-01T00:00:00", "status": "new"})
        o = from_json(Order, text)
        self.assertEqual((o.items, o.tags, o.shipped, o.priority, o.note), ([], [], None, Priority.LOW, None))
        self.assertIsNone(o.customer.address.zip)
        self.assertFalse(o.customer.vip)
        o.items.append(1)
        self.assertEqual(from_json(Order, text).items, [])
        self.assertIsNone(json.loads(to_json(o))["note"])

    def test_recursive_and_future_annotations(self):
        n = Node(
            "root",
            [Leaf(1), Leaf(2, datetime(2020, 2, 29, 23, 59, 59))],
            child=Node("kid", [], colors=[Color.GREEN, Color.RED], scores=[1.5, None, 2.0], maybe=[1, 2], level=Level.TWO),
        )
        data = json.loads(to_json(n))
        self.assertEqual(data["child"]["colors"], ["g", "r"])
        self.assertEqual(data["child"]["level"], 2)
        self.assertEqual(data["child"]["child"], None)
        self.assertEqual(from_json(Node, to_json(n)), n)

    def test_int_accepted_for_float(self):
        n = from_json(Node, '{"name": "a", "leaves": [], "ratio": 3, "scores": [1, null]}')
        self.assertIsInstance(n.ratio, float)
        self.assertEqual(n.ratio, 3.0)
        self.assertIsInstance(n.scores[0], float)

    def test_wrong_types(self):
        base = {"sku": "a", "qty": 1, "price": 1.0}
        self.assertBad(LineItem, {**base, "qty": "1"}, "qty")
        self.assertBad(LineItem, {**base, "qty": True}, "qty")
        self.assertBad(LineItem, {**base, "qty": 1.5}, "qty")
        self.assertBad(LineItem, {**base, "price": "1.0"}, "price")
        self.assertBad(LineItem, {**base, "price": False}, "price")
        self.assertBad(LineItem, {**base, "sku": 5}, "sku")
        self.assertBad(Customer, {"id": 1, "name": "n", "address": {"street": "s", "city": "c"}, "vip": 1}, "vip")
        self.assertBad(Customer, {"id": 1, "name": "n", "address": "Paris"}, "address")
        self.assertBad(Node, {"name": "a", "leaves": {"n": 1}}, "leaves")
        self.assertBad(Node, {"name": "a", "leaves": [{"n": "1"}]}, "n")
        self.assertBad(Node, {"name": "a", "leaves": [], "maybe": [1, None]}, "maybe")

    def test_null_handling(self):
        self.assertBad(LineItem, {"sku": None, "qty": 1, "price": 1.0}, "sku")
        self.assertBad(Node, {"name": "a", "leaves": None}, "leaves")
        self.assertBad(Node, {"name": "a", "leaves": [], "colors": [None]}, "colors")
        self.assertIsNone(from_json(Node, '{"name": "a", "leaves": [], "maybe": null, "child": null}').maybe)

    def test_unknown_and_missing(self):
        self.assertBad(LineItem, {"sku": "a", "qty": 1}, "price")
        self.assertBad(Node, {"name": "a", "leaves": [{"n": 1, "extra": 2}]}, "extra")
        self.assertBad(Node, {"name": "a", "leaves": [], "child": {"name": "b", "leaves": [], "bogus": 1}}, "bogus")
        self.assertBad(Customer, {"id": 1, "name": "n", "address": {"street": "s", "city": "c", "country": "FR"}}, "country")

    def test_enum_and_datetime_errors(self):
        self.assertBad(Node, {"name": "a", "leaves": [], "colors": ["blue"]}, "colors")
        self.assertBad(Node, {"name": "a", "leaves": [], "colors": ["RED"]})
        self.assertBad(Node, {"name": "a", "leaves": [], "level": "1"}, "level")
        self.assertBad(Node, {"name": "a", "leaves": [{"n": 1, "when": "yesterday"}]}, "when")
        self.assertBad(Node, {"name": "a", "leaves": [{"n": 1, "when": 1700000000}]}, "when")

    def test_bad_documents(self):
        self.assertBad(LineItem, "{not json")
        self.assertBad(LineItem, "[1, 2]")
        self.assertBad(LineItem, '"x"')


if __name__ == "__main__":
    unittest.main()
