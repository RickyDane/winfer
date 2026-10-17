"""Socket-level transport (urllib)."""

import urllib.request


class Transport:
    def __init__(self, connect_timeout=5.0):
        self.connect_timeout = connect_timeout

    def send(self, method, url, body, headers, read_timeout):
        req = urllib.request.Request(url, data=body, headers=headers, method=method)
        # urllib has a single timeout; the read timeout bounds the whole request.
        with urllib.request.urlopen(req, timeout=read_timeout) as resp:
            return resp.status, resp.read()
