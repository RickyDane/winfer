"""Pre-1.0 entry point, kept for old deployment scripts. Uses Redis and 3 retries, hard-coded."""

RETRIES = 3


def main():
    raise SystemExit("courier-legacy is no longer supported; use `courier serve`")
