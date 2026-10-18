from .base import Middleware


class DedupeMiddleware(Middleware):
    def __init__(self, next_handler):
        super().__init__(next_handler)
        self.seen = set()

    def __call__(self, msg):
        if msg.get("id") in self.seen:
            return None
        self.seen.add(msg.get("id"))
        return self.next(msg)
