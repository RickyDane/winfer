from dataclasses import dataclass, field

ACTIVE = "active"
CANCELLED = "cancelled"
FULFILLED = "fulfilled"
STATUSES = (ACTIVE, CANCELLED, FULFILLED)


@dataclass
class Item:
    sku: str
    on_hand: int = 0
    reserved: int = 0


@dataclass
class Reservation:
    id: int
    sku: str
    qty: int
    status: str = ACTIVE
    history: list = field(default_factory=list)
