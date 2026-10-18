"""HTTP client facade over a transport callable(method, url, timeout, body)."""


class Client:
    def __init__(self, transport):
        self.transport = transport

    def request(self, method, url, *, timeout=None, body=None):
        return self.transport(method, url, timeout, body)

    def fetch_all(self, urls, timeout):
        """GETs several URLs; returns the responses in order."""
        return [self.request("GET", u, timeout=timeout) for u in urls]
