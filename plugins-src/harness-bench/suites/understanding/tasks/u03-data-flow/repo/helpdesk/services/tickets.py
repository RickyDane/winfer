from ..db import tickets_repo
from .status import Status

SORT_COLUMNS = {"newest": "created_at DESC", "oldest": "created_at ASC", "priority": "priority DESC"}


def search(filters, text, sort, limit):
    status = Status(filters["status"]).value if "status" in filters else None
    order = SORT_COLUMNS.get(sort, "created_at DESC")
    return tickets_repo.find(
        region=filters.get("region"),
        status=status,
        owner=_owner_prefix(filters.get("owner")),
        text=text,
        order=order,
        limit=limit,
    )


def _owner_prefix(owner):
    if not owner:
        return None
    # Logins are case-insensitive; match "alice" for "Alice.Smith".
    return owner.lower().split(".")[0]


def create(title, note, reporter):
    return tickets_repo.insert(title=title, note=note, reporter=reporter)
