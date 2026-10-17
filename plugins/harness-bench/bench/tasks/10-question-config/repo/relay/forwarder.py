"""Reads payloads (one JSON object per line) from stdin and forwards them."""

import json
import sys


class Forwarder:
    def __init__(self, client, upstream, retries=2):
        self.client = client
        self.upstream = upstream.rstrip("/")
        self.retries = retries

    def forward(self, payload):
        body = json.dumps(payload).encode()
        last = None
        for _ in range(self.retries + 1):
            try:
                return self.client.post(self.upstream + "/hooks", body)
            except OSError as e:
                last = e
        raise last

    def run_stdin(self):
        for line in sys.stdin:
            if line.strip():
                self.forward(json.loads(line))
        return 0
