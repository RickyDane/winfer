"""Fake transport so the project runs without a network."""


class Response:
    def __init__(self, status, body):
        self.status = status
        self.body = body


class Transport:
    def __init__(self, timeout_s, proxy=""):
        self.timeout_s = timeout_s
        self.proxy = proxy
        self.sent = []

    def send(self, method, url, headers, body=None):
        self.sent.append((method, url, headers, body))
        return Response(200, {})
