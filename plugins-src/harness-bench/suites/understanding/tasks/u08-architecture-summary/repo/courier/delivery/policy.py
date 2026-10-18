class RetryPolicy:
    """How often and how fast to retry a failed delivery."""

    def __init__(self, settings):
        self.max_retries = int(settings.get("delivery.max_retries", 4))
        self.backoff = float(settings.get("delivery.backoff", 1.0))

    def delay(self, attempt):
        return self.backoff * (2 ** (attempt - 1))
