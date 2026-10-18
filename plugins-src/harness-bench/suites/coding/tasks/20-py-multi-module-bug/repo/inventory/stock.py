from . import ledger


def available(store, sku):
    item = store.item(sku)
    return item.on_hand - item.reserved


def restock(store, sku, qty):
    if qty <= 0:
        raise ValueError("qty must be positive")
    item = store.item(sku)
    item.on_hand += qty
    # Resync in case reservations changed elsewhere.
    ledger.recount(store, sku)
    return item.on_hand


def adjust(store, sku, on_hand):
    """Stock count correction: sets on_hand to the counted number."""
    item = store.item(sku)
    item.on_hand = on_hand
    ledger.recount(store, sku)
    return available(store, sku)
