from .models import ACTIVE, CANCELLED, FULFILLED, Reservation
from .stock import available


def reserve(store, sku, qty):
    if qty <= 0:
        raise ValueError("qty must be positive")
    if available(store, sku) < qty:
        raise ValueError(f"not enough {sku}")
    res = Reservation(store.new_reservation_id(), sku, qty)
    res.history.append("reserved")
    store.reservations[res.id] = res
    store.item(sku).reserved += qty
    return res


def cancel(store, res_id):
    res = store.reservations[res_id]
    if res.status != ACTIVE:
        raise ValueError(f"reservation {res_id} is {res.status}")
    res.status = CANCELLED
    res.history.append("cancelled")
    store.item(res.sku).reserved -= res.qty
    return res


def fulfill(store, res_id):
    res = store.reservations[res_id]
    if res.status != ACTIVE:
        raise ValueError(f"reservation {res_id} is {res.status}")
    res.status = FULFILLED
    res.history.append("fulfilled")
    item = store.item(res.sku)
    item.reserved -= res.qty
    item.on_hand -= res.qty
    return res
