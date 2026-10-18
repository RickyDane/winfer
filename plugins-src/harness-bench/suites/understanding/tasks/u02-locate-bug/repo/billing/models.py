"""Order and line models."""

from enum import Enum

from .money import to_money


class TaxState(Enum):
    PENDING = "pending"
    APPLIED = "applied"
    EXEMPT = "exempt"


class Line:
    __slots__ = ("sku", "qty", "unit_price", "tax_state", "note")

    def __init__(self, sku, qty, unit_price, tax_state=TaxState.PENDING, note=""):
        self.sku = sku
        self.qty = qty
        self.unit_price = to_money(unit_price)
        self.tax_state = tax_state
        self.note = note

    @property
    def total(self):
        return to_money(self.unit_price * self.qty)

    def copy_with(self, **changes):
        """Return a copy of this line with some fields changed; everything else is kept."""
        return Line(
            sku=changes.get("sku", self.sku),
            qty=changes.get("qty", self.qty),
            unit_price=changes.get("unit_price", self.unit_price),
            tax_state=changes.get("tax_state", TaxState.PENDING),
            note=changes.get("note", self.note),
        )

    def __repr__(self):
        return f"Line({self.sku!r}, {self.qty}, {self.unit_price}, {self.tax_state.name})"


class Order:
    def __init__(self, customer, region, lines, code=None):
        self.customer = customer
        self.region = region
        self.lines = list(lines)
        self.code = code
