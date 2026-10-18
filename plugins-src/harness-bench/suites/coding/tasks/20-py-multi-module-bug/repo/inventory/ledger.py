"""Derived numbers. The reservations are the source of truth for `Item.reserved`."""

from .models import CANCELLED, FULFILLED


def open_reservations(store, sku):
    return [r for r in store.reservations.values() if r.sku == sku and r.status not in (CANCELLED, FULFILLED)]


def recount(store, sku):
    item = store.item(sku)
    item.reserved = sum(r.qty for r in open_reservations(store, sku))
    return item.reserved
