import time

from .base import Middleware


class RateLimitMiddleware(Middleware):
    def __init__(self, next_handler, per_second=100):
        super().__init__(next_handler)
        self.min_gap = 1 / per_second
        self.last = 0.0

    def __call__(self, msg):
        now = time.monotonic()
        if now - self.last < self.min_gap:
            time.sleep(self.min_gap - (now - self.last))
        self.last = time.monotonic()
        return self.next(msg)
