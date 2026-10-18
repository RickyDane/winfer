import unicodedata

from vendor.slugkit import slugify

MAX_SLUG = 60


def _ascii_fold(text):
    decomposed = unicodedata.normalize("NFKD", text)
    return "".join(c for c in decomposed if not unicodedata.combining(c))


def make_slug(title):
    """URL slug for an article title."""
    return slugify(_ascii_fold(title), max_length=MAX_SLUG)
