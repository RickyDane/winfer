"""Page-number pagination (pages are 1-based)."""

from dataclasses import dataclass


@dataclass
class Page:
    items: list
    page: int
    per_page: int
    total: int
    total_pages: int
    has_prev: bool
    has_next: bool


def paginate(items, page=1, per_page=10):
    """Returns page `page` of `items`.

    `per_page` must be >= 1 and `page` >= 1, otherwise ValueError. `total_pages` is the
    number of pages needed for all items (0 when there are none). A page past the end has
    no items. `has_prev` is True for every page after the first; `has_next` is True when a
    later page has items.
    """
    if per_page < 1 or page < 1:
        raise ValueError("page and per_page must be >= 1")
    total = len(items)
    total_pages = total // per_page
    start = page * per_page
    return Page(
        items=list(items[start:start + per_page]),
        page=page,
        per_page=per_page,
        total=total,
        total_pages=total_pages,
        has_prev=page > 1,
        has_next=page <= total_pages,
    )
