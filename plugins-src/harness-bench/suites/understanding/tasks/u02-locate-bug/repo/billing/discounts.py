"""Discount codes. Discounts apply to gross (VAT-inclusive) prices, which is why they run after
VAT in the pipeline."""

from decimal import Decimal

from .money import pct, to_money

CODES = {
    "SPRING10": 10,
    "VIP25": 25,
    "FREESHIP": 0,
}


def discount_rate(code):
    if code is None:
        return Decimal(0)
    return pct(CODES.get(code.upper(), 0))


def apply_discount(order):
    rate = discount_rate(order.code)
    if not rate:
        return order
    order.lines = [
        line.copy_with(unit_price=to_money(line.unit_price * (1 - rate)), note=f"-{order.code}")
        for line in order.lines
    ]
    return order
