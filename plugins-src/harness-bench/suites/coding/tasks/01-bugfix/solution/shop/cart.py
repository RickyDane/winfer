from dataclasses import dataclass, field

from .pricing import apply_discount, with_tax


@dataclass
class Line:
    sku: str
    unit_cents: int
    qty: int = 1


@dataclass
class Cart:
    lines: list = field(default_factory=list)
    discount_percent: int = 0

    def add(self, sku: str, unit_cents: int, qty: int = 1) -> None:
        """Add an item; adding a SKU that's already in the cart increases its quantity."""
        if qty <= 0:
            raise ValueError("qty must be positive")
        for line in self.lines:
            if line.sku == sku:
                line.qty += qty
                return
        self.lines.append(Line(sku, unit_cents, qty))

    def remove(self, sku: str) -> None:
        self.lines = [l for l in self.lines if l.sku != sku]

    def subtotal(self) -> int:
        return sum(l.unit_cents * l.qty for l in self.lines)

    def total(self) -> int:
        """Subtotal, minus the discount, plus tax."""
        return with_tax(apply_discount(self.subtotal(), self.discount_percent))
