from . import events
from .registry import on


@on(events.REFUND_ISSUED)
def refund_issued(msg):
    return {"notify": "accounting", "refund": msg["id"]}
