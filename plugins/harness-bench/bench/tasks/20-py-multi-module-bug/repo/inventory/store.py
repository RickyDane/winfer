import json

from .models import Item, Reservation
from . import ledger


class Store:
    def __init__(self):
        self.items = {}
        self.reservations = {}
        self._next_id = 1

    def item(self, sku):
        if sku not in self.items:
            self.items[sku] = Item(sku)
        return self.items[sku]

    def new_reservation_id(self):
        rid = self._next_id
        self._next_id += 1
        return rid

    def dump(self):
        return json.dumps(
            {
                "items": {s: i.on_hand for s, i in self.items.items()},
                "reservations": [
                    {"id": r.id, "sku": r.sku, "qty": r.qty, "status": r.status} for r in self.reservations.values()
                ],
                "next_id": self._next_id,
            }
        )

    @classmethod
    def load(cls, text):
        data = json.loads(text)
        store = cls()
        for sku, on_hand in data["items"].items():
            store.item(sku).on_hand = on_hand
        for r in data["reservations"]:
            store.reservations[r["id"]] = Reservation(r["id"], r["sku"], r["qty"], r["status"])
        store._next_id = data["next_id"]
        # `reserved` isn't stored; rebuild it from the reservations.
        for sku in store.items:
            ledger.recount(store, sku)
        return store
