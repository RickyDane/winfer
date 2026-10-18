import uuid

from .base import Middleware


class TracingMiddleware(Middleware):
    def __call__(self, msg):
        msg.setdefault("trace_id", uuid.uuid4().hex)
        return self.next(msg)
