"""VAT. apply_vat() is idempotent: lines that already carry VAT are left alone, so the pipeline
can safely call it again after later steps (see pipeline.py)."""

from .models import TaxState
from .money import pct, to_money
from .regions import vat_rate


def apply_vat(order):
    rate = pct(vat_rate(order.region))
    lines = []
    for line in order.lines:
        if line.tax_state is TaxState.PENDING:
            gross = to_money(line.unit_price * (1 + rate))
            line = line.copy_with(unit_price=gross, tax_state=TaxState.APPLIED)
        lines.append(line)
    order.lines = lines
    return order


def apply_vat_legacy(order):
    # Old implementation, kept for the 2019 invoice re-export script. Not used by the pipeline.
    rate = pct(vat_rate(order.region))
    for line in order.lines:
        line.unit_price = to_money(line.unit_price * (1 + rate) * (1 + rate))
    return order


def mark_exempt(order, skus):
    order.lines = [l.copy_with(tax_state=TaxState.EXEMPT) if l.sku in skus else l for l in order.lines]
    return order
