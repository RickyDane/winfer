"""Thin HTTP client over the transport."""

from relay.net.transport import Transport

DEFAULT_TIMEOUT = 10.0


class HttpClient:
    def __init__(self, transport, timeout=DEFAULT_TIMEOUT, user_agent="relay"):
        self.transport = transport
        self.timeout = timeout
        self.user_agent = user_agent

    @classmethod
    def from_settings(cls, settings):
        http = settings.http
        transport = Transport(connect_timeout=http.connect_timeout)
        return cls(transport, timeout=min(http.request_timeout, http.max_timeout), user_agent=http.user_agent)

    def post(self, url, body):
        headers = {"User-Agent": self.user_agent, "Content-Type": "application/json"}
        return self.transport.send("POST", url, body, headers, read_timeout=self.timeout)
