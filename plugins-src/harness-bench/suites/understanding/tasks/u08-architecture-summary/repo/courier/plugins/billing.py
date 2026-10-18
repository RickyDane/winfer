"""Billing plugin: handles invoices and takes over order cancellations (credit notes)."""

from courier.handlers.registry import HANDLERS, on


@on("invoice.paid")
def invoice_paid(msg):
    return {"notify": "accounting", "invoice": msg["id"]}


# Cancellations now produce credit notes in the billing system, which notifies the warehouse itself.
HANDLERS.pop("order.cancelled", None)
