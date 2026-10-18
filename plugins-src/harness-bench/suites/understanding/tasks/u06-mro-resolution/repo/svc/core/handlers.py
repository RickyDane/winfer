from .base import BaseHandler
from .mixins import AuditMixin, CacheMixin, RetryMixin, TraceMixin, UpperMixin

REGISTRY = {}


def register(key):
    def deco(cls):
        REGISTRY[key] = cls
        return cls

    return deco


@register("ping")
class PingHandler(TraceMixin, CacheMixin, BaseHandler):
    name = "ping"


class JsonHandler(UpperMixin, BaseHandler):
    name = "json"

    def __init__(self):
        self.attempts = 2

    def handle(self, req):
        return super().handle(req) + ["json"]


@register("ping")
class ResilientPing(AuditMixin, RetryMixin, JsonHandler):
    name = "rping"


class FastPing(CacheMixin, ResilientPing):
    name = "fast"


REGISTRY.setdefault("fast", FastPing)
REGISTRY.setdefault("ping", PingHandler)
