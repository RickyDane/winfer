from collections import Counter

from .models import STATUSES
from . import ledger


def summary(store):
    """{sku: {"on_hand", "reserved", "available", "open"}} plus "_status": counts per status."""
    out = {}
    for sku, item in sorted(store.items.items()):
        out[sku] = {
            "on_hand": item.on_hand,
            "reserved": item.reserved,
            "available": item.on_hand - item.reserved,
            "open": len(ledger.open_reservations(store, sku)),
        }
    counts = Counter(r.status for r in store.reservations.values())
    out["_status"] = {s: counts.get(s, 0) for s in STATUSES}
    return out
