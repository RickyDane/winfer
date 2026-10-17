"""Small text helpers used by the CMS. See README.md for the spec."""

import re

ELLIPSIS = "…"


def slugify(text):
    """URL slug: lowercase ASCII letters/digits separated by single hyphens."""
    slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return slug or "n-a"


def truncate(text, limit):
    """Shorten text to at most `limit` characters, ending with an ellipsis if cut."""
    if limit < 1:
        raise ValueError("limit must be >= 1")
    if len(text) <= limit:
        return text
    return text[: limit - 1] + ELLIPSIS


def equals_ignore_case(a, b):
    """Case-insensitive comparison."""
    return a.lower() == b.lower()


def word_count(text):
    """Number of words in text."""
    return len(re.findall(r"[A-Za-z0-9']+", text))
