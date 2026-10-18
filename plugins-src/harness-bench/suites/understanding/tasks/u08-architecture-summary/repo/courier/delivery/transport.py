import json
import socket


class TransientError(Exception):
    """Delivery failed but may succeed if retried."""


class TcpTransport:
    def __init__(self, host="downstream", port=7700):
        self.addr = (host, port)

    def send(self, payload):
        try:
            with socket.create_connection(self.addr, timeout=5) as s:
                s.sendall(json.dumps(payload).encode() + b"\n")
        except OSError as e:
            raise TransientError(str(e)) from e
