"""Invoice totals."""

from .money import to_money
from .pipeline import run


def invoice_total(order):
    order = run(order)
    # BUG? VAT counted twice here? -> no: line totals are gross already, we only sum them.
    total = sum((line.total for line in order.lines), to_money(0))
    return to_money(total)


def invoice_lines(order):
    order = run(order)
    return [(l.sku, l.qty, str(l.unit_price), l.note) for l in order.lines]
