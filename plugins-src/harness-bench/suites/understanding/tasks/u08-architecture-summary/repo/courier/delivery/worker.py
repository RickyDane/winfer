import time

from .transport import TcpTransport, TransientError


class Worker:
    def __init__(self, policy, store, transport=None):
        self.policy = policy
        self.store = store
        self.transport = transport or TcpTransport()

    def deliver(self, payload):
        attempt = 0
        while True:
            try:
                return self.transport.send(payload)
            except TransientError:
                attempt += 1
                if attempt > self.policy.max_retries:
                    self.store.put(f"dead:{id(payload)}", repr(payload))
                    raise
                time.sleep(self.policy.delay(attempt))
