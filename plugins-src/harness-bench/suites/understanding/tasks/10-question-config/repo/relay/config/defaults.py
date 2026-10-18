"""Built-in defaults (the lowest layer)."""

DEFAULTS = {
    "upstream": "http://localhost:9000",
    "log_level": "info",
    "retries": 2,
    "http": {
        "connect_timeout": 5.0,
        "request_timeout": 30.0,
        "max_timeout": 300.0,
        "user_agent": "relay/1.4",
    },
}
