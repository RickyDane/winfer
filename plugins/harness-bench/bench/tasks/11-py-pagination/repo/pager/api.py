"""Cursor-paginated list endpoint."""

from pager import cursor

DEFAULT_LIMIT = 20
MAX_LIMIT = 100


def list_items(items, after=None, limit=None):
    """One page of `items` for a cursor API.

    `limit` defaults to DEFAULT_LIMIT and is capped at MAX_LIMIT; a limit below 1 raises
    ValueError. `after` is the `next` cursor from a previous response (None = start).
    Returns {"data": [...], "next": cursor or None}; `next` is None when there is nothing
    after this page. Following `next` until it is None yields every item exactly once.
    A cursor whose offset is negative or not an int raises cursor.InvalidCursor.
    """
    limit = DEFAULT_LIMIT if limit is None else limit
    if limit < 1:
        raise ValueError("limit must be >= 1")
    limit = max(limit, MAX_LIMIT)
    offset = 0
    if after is not None:
        offset = cursor.decode(after).get("offset", 0)
    data = items[offset:offset + limit]
    end = offset + limit
    nxt = cursor.encode({"offset": end - 1}) if end <= len(items) else None
    return {"data": list(data), "next": nxt}
