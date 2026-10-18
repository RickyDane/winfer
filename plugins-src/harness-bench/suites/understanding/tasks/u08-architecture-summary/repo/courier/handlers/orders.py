from . import events
from .registry import on


@on(events.ORDER_CREATED)
def order_created(msg):
    return {"notify": "warehouse", "order": msg["id"]}


@on(events.ORDER_CANCELLED)
def order_cancelled(msg):
    return {"notify": "warehouse", "cancel": msg["id"]}
