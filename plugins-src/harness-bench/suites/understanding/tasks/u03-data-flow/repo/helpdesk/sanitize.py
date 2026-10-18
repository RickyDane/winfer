"""Input cleaning helpers."""

import re

_SAFE = re.compile(r"[^A-Za-z0-9 _.@-]")


def sql_safe(value):
    """Strip every character that could break out of a SQL string literal."""
    return _SAFE.sub("", str(value))


def quote(value):
    """Render a value as a SQL string literal (single quotes doubled)."""
    return "'" + str(value).replace("'", "''") + "'"


def clamp_limit(value, default=50, maximum=200):
    try:
        n = int(value)
    except (TypeError, ValueError):
        return default
    return max(1, min(n, maximum))
