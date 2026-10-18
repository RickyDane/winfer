"""Checkout totals. See SPEC.md."""

SHIPPING_CENTS = 499
FREE_SHIPPING_FROM = 5000
VAT_PERCENT = 20


def subtotal(cart):
    return sum(line["unit_price"] * line["qty"] for line in cart)


def round_half_up(numerator, denominator):
    """Integer division rounded half up (non-negative inputs)."""
    return (2 * numerator + denominator) // (2 * denominator)


def checkout_total(cart, coupon=None):
    """Total in cents for `cart`, per SPEC.md."""
    raise NotImplementedError
