from decimal import ROUND_HALF_UP, Decimal

CENT = Decimal("0.01")


def to_money(value):
    return Decimal(str(value)).quantize(CENT, rounding=ROUND_HALF_UP)


def pct(rate):
    """pct(20) -> Decimal('0.20')"""
    return Decimal(str(rate)) / Decimal(100)
