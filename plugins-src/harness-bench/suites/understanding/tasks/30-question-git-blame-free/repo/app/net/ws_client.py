"""Change-event stream over a websocket (simulated)."""

from app.net.retry import Backoff


class EventStream:
    def __init__(self, settings):
        self.url = settings.get("ws.url")
        self.reconnect = Backoff(
            settings.get_int("ws.reconnect.backoff"), attempts=10, max_ms=settings.get_int("ws.reconnect.max_delay_ms")
        )
        self.published = []

    def publish(self, topic, payload):
        self.published.append((topic, payload))
