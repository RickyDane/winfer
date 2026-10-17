"""NOK money formatting."""

from decimal import Decimal

from ledger.core.grouping import group_digits
from ledger.core.rounding import quantize

CODE = "NOK"
SYMBOL = ' kr'
DECIMALS = 2
SYMBOL_AFTER = True
THOUSANDS = ' '
DECIMAL_POINT = ','
ROUNDING = "HALF_UP"


def round_amount(value):
    q = quantize(value, DECIMALS, ROUNDING)
    return q


def format_amount(value):
    """value: Decimal (major units). Returns e.g. '1 234,50 kr'."""
    q = round_amount(Decimal(value))
    sign = "-" if q < 0 else ""
    whole, _, frac = f"{abs(q):.{DECIMALS}f}".partition(".")
    text = group_digits(whole, THOUSANDS) + (DECIMAL_POINT + frac if DECIMALS else "")
    return sign + (text + SYMBOL if SYMBOL_AFTER else SYMBOL + text)
