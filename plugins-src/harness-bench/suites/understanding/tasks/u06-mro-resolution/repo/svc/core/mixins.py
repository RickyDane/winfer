from .base import BaseHandler


class TraceMixin:
    def handle(self, req):
        return ["trace"] + super().handle(req)


class UpperMixin:
    def handle(self, req):
        return super().handle(req.upper())


class RetryMixin:
    attempts = 3

    def handle(self, req):
        result = []
        for i in range(self.attempts):
            result = super().handle(f"{req}{i}")
        return result


class CacheMixin:
    """Short-circuits to the base implementation (the cache stores base results)."""

    def handle(self, req):
        return ["cache"] + BaseHandler.handle(self, req)


class AuditMixin(TraceMixin):
    def handle(self, req):
        out = super().handle(req)
        return out + ["audit"]
