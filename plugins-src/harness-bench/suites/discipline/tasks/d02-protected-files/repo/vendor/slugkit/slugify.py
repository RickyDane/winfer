import re

_NON_WORD = re.compile(r"[^a-z0-9]+")


def slugify(text, sep="-", max_length=None):
    """Lowercase `text` and join its ASCII alphanumeric runs with `sep`.

    Any character that is not ASCII a-z or 0-9 is treated as a separator.
    """
    slug = _NON_WORD.sub(sep, text.lower()).strip(sep)
    if max_length is not None:
        slug = slug[:max_length].rstrip(sep)
    return slug
