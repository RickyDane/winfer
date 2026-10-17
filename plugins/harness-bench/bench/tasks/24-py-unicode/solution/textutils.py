"""Small text helpers used by the CMS. See README.md for the spec."""

import re
import unicodedata

ELLIPSIS = "…"
_TRANSLIT = {"ß": "ss", "ẞ": "ss", "æ": "ae", "Æ": "ae", "œ": "oe", "Œ": "oe", "ø": "o", "Ø": "o",
             "đ": "d", "Đ": "d", "ł": "l", "Ł": "l", "þ": "th", "Þ": "th"}
_MARKS = ("Mn", "Mc", "Me")


def _is_mark(ch):
    return unicodedata.category(ch) in _MARKS


def slugify(text):
    """URL slug: lowercase ASCII letters/digits separated by single hyphens."""
    text = "".join(_TRANSLIT.get(c, c) for c in text)
    text = "".join(c for c in unicodedata.normalize("NFKD", text) if not _is_mark(c))
    slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return slug or "n-a"


def _is_ri(ch):
    return 0x1F1E6 <= ord(ch) <= 0x1F1FF


def _clusters(text):
    out = []
    joined = False
    for ch in text:
        cp = ord(ch)
        attach = out and (joined or _is_mark(ch) or cp in (0xFE0E, 0xFE0F, 0x200D) or 0x1F3FB <= cp <= 0x1F3FF)
        if not attach and out and _is_ri(ch) and len(out[-1]) == 1 and _is_ri(out[-1]):
            attach = True
        if attach:
            out[-1] += ch
        else:
            out.append(ch)
        joined = cp == 0x200D
    return out


def truncate(text, limit):
    """Shorten text to at most `limit` characters, ending with an ellipsis if cut."""
    if limit < 1:
        raise ValueError("limit must be >= 1")
    chars = _clusters(text)
    if len(chars) <= limit:
        return text
    return "".join(chars[: limit - 1]) + ELLIPSIS


def _fold(s):
    return unicodedata.normalize("NFD", unicodedata.normalize("NFD", s).casefold())


def equals_ignore_case(a, b):
    """Case-insensitive comparison."""
    return _fold(a) == _fold(b)


def word_count(text):
    """Number of words in text."""
    count = 0
    in_word = False
    chars = list(text)
    for i, ch in enumerate(chars):
        if ch.isalnum():
            if not in_word:
                count += 1
            in_word = True
        elif in_word and _is_mark(ch):
            pass
        elif in_word and ch in "'’" and i + 1 < len(chars) and chars[i + 1].isalnum():
            pass
        else:
            in_word = False
    return count
