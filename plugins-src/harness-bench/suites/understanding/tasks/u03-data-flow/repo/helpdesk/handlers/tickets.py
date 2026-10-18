"""Ticket endpoints. Request bodies are JSON objects (request.json)."""

import logging

from ..sanitize import clamp_limit, sql_safe
from ..services import tickets as service

log = logging.getLogger(__name__)

# Public API field names -> internal filter names.
FILTER_FIELDS = {"area": "region", "state": "status", "assignee": "owner"}


def search(request):
    body = request.json
    filters = {FILTER_FIELDS[k]: v for k, v in body.items() if k in FILTER_FIELDS}
    # Every filter value is cleaned before it goes anywhere near the database.
    for name, value in filters.items():
        value = sql_safe(value)
    text = body.get("q", "").strip()
    return service.search(
        filters,
        text=text,
        sort=body.get("order_by"),
        limit=clamp_limit(body.get("limit")),
    )


def create(request):
    body = request.json
    title = body["title"].strip()
    note = body.get("note", "")
    log.info(f"creating ticket {title!r} with note {note!r}")
    return service.create(title=title, note=note, reporter=request.user_id)
