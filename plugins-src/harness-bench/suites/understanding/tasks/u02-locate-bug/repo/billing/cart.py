"""Cart normalisation: merge duplicate SKUs before pricing."""


def merge_duplicates(order):
    merged = {}
    for line in order.lines:
        if line.sku in merged:
            prev = merged[line.sku]
            merged[line.sku] = prev.copy_with(qty=prev.qty + line.qty)
        else:
            merged[line.sku] = line
    order.lines = list(merged.values())
    return order


def drop_empty(order):
    order.lines = [l for l in order.lines if l.qty > 0]
    return order
