"""v1 API client. Deprecated: nothing creates it anymore; kept for the migration scripts."""

from app.net.retry import Backoff
from app.net.transport import Transport


class LegacyHttpClient:
    def __init__(self, settings):
        self.transport = Transport(settings.get_float("net.timeout_s"))
        self.retry = Backoff(settings.get_int("http.retry_backoff_ms"), 3)

    def get(self, path):
        return self.retry.run(lambda: self.transport.send("GET", "/v1/" + path, {}))
