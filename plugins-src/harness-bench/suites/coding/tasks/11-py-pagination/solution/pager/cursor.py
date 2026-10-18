"""Opaque cursors: URL-safe base64 of compact JSON, without '=' padding."""

import base64
import json


class InvalidCursor(ValueError):
    pass


def encode(state):
    raw = json.dumps(state, separators=(",", ":"), sort_keys=True).encode()
    return base64.urlsafe_b64encode(raw).decode().rstrip("=")


def decode(cursor):
    """Inverse of encode. Raises InvalidCursor for anything encode can't have produced."""
    try:
        if not isinstance(cursor, str) or "=" in cursor:
            raise ValueError
        raw = base64.urlsafe_b64decode(cursor + "=" * (-len(cursor) % 4))
        state = json.loads(raw)
    except Exception:
        raise InvalidCursor(cursor) from None
    if not isinstance(state, dict):
        raise InvalidCursor(cursor)
    return state
