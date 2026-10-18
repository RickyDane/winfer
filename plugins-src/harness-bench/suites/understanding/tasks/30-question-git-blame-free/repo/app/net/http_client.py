"""The HTTP API client used by all services."""

from app.net.retry import policy_for
from app.net.transport import Transport


class HttpClient:
    def __init__(self, settings):
        self.base_url = settings.get("http.base_url")
        self.headers = {"User-Agent": settings.get("http.user_agent")}
        self.transport = Transport(settings.get_float("http.timeout"), settings.get("net.proxy"))
        self.retry = policy_for(settings, "http")

    def request(self, method, path, body=None):
        url = self.base_url.rstrip("/") + "/" + path.lstrip("/")
        return self.retry.run(lambda: self.transport.send(method, url, self.headers, body))

    def get(self, path):
        return self.request("GET", path)

    def post(self, path, body):
        return self.request("POST", path, body)
