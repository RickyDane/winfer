"""The billing pipeline. Order of steps matters:

1. cart normalisation
2. VAT (prices become gross)
3. discounts (on gross prices)
4. VAT again: a no-op for lines that already carry VAT, but catches lines added or re-priced
   by steps 3+ that are still PENDING.
"""

from . import cart, discounts, tax

STEPS = [
    cart.merge_duplicates,
    cart.drop_empty,
    tax.apply_vat,
    discounts.apply_discount,
    tax.apply_vat,
]


def run(order):
    for step in STEPS:
        order = step(order)
    return order
