from app.settings.registry import alias, register

register("http.base_url", "https://api.example.test/v2")
register("http.user_agent", "sync/2.4")
register("http.backoff_ms", 500, doc="Backoff between HTTP retries.")
register("http.retry_backoff_ms", 1000, doc="Deprecated: used by the v1 client only.")
register("http.max_connections", 16)
alias("http.retry.backoff", "http.backoff_ms")
alias("http.retry.attempts", "retry.attempts")
