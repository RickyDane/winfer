from vendor.slugkit import slugify

MAX_SLUG = 60


def make_slug(title):
    """URL slug for an article title."""
    return slugify(title, max_length=MAX_SLUG)
