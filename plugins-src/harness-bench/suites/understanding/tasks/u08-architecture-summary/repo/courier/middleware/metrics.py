import time

from .base import Middleware


class MetricsMiddleware(Middleware):
    timings = []

    def __call__(self, msg):
        t = time.perf_counter()
        try:
            return self.next(msg)
        finally:
            self.timings.append(time.perf_counter() - t)
