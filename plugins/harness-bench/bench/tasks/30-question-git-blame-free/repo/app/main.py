"""Entry point: wires settings, clients and services together."""

from app.net.http_client import HttpClient
from app.net.ws_client import EventStream
from app.services.billing import BillingSync
from app.services.search import SearchIndexer
from app.services.users import UserSync
from app.settings import Settings


def build(overrides=None):
    settings = Settings(overrides or {})
    http = HttpClient(settings)
    events = EventStream(settings)
    return {
        "settings": settings,
        "http": http,
        "events": events,
        "users": UserSync(http),
        "billing": BillingSync(http, events),
        "search": SearchIndexer(http),
    }


if __name__ == "__main__":
    app = build()
    print("retry delays (ms):", [app["http"].retry.delay_ms(n) for n in range(app["http"].retry.attempts)])
