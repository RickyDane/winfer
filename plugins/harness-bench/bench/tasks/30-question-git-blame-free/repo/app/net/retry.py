"""Retry policies."""

import time


class Backoff:
    """Exponential backoff: base, 2*base, 4*base, ... capped at max_ms."""

    def __init__(self, base_ms, attempts, max_ms=None):
        self.base_ms = base_ms
        self.attempts = attempts
        self.max_ms = max_ms

    def delay_ms(self, retry):
        d = self.base_ms * (2 ** retry)
        return min(d, self.max_ms) if self.max_ms else d

    def run(self, fn, retry_on=(IOError,)):
        for n in range(self.attempts):
            try:
                return fn()
            except retry_on:
                if n == self.attempts - 1:
                    raise
                time.sleep(self.delay_ms(n) / 1000)


def policy_for(settings, kind, max_ms=None):
    """The retry policy for a client kind ("http", "search", ...)."""
    return Backoff(settings.get_int(f"{kind}.retry.backoff"), settings.get_int(f"{kind}.retry.attempts"), max_ms)
