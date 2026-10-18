from __future__ import annotations

import enum
from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional


class Status(enum.Enum):
    NEW = "new"
    PAID = "paid"
    SHIPPED = "shipped"


class Priority(enum.IntEnum):
    LOW = 1
    HIGH = 2


@dataclass
class Address:
    street: str
    city: str
    zip: Optional[str] = None


@dataclass
class Customer:
    id: int
    name: str
    address: Address
    vip: bool = False


@dataclass
class LineItem:
    sku: str
    qty: int
    price: float


@dataclass
class Order:
    id: int
    customer: Customer
    created: datetime
    status: Status
    items: list[LineItem] = field(default_factory=list)
    shipped: datetime | None = None
    priority: Priority = Priority.LOW
    tags: list[str] = field(default_factory=list)
    note: Optional[str] = None
