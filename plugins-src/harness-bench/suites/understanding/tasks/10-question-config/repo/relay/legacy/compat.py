"""Pre-1.0 helpers. Not imported anywhere anymore; kept for plugins that vendored them."""

import os


def request_timeout():
    # 0.x read REQUEST_TIMEOUT directly.
    return float(os.environ.get("REQUEST_TIMEOUT", "30"))


def upstream():
    return os.environ.get("RELAY_UPSTREAM_URL", "http://localhost:9000")
