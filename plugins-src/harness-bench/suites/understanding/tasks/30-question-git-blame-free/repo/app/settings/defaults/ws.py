from app.settings.registry import alias, register

register("ws.url", "wss://events.example.test/stream")
register("ws.ping_interval_s", 25)
register("ws.reconnect.max_delay_ms", 30_000)
# The event stream reconnects with the same backoff the HTTP API used to document.
alias("ws.reconnect.backoff", "http.backoff_ms")
