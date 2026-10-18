"""Environment layer.

Variables named <PREFIX><KEY> set top-level keys; a double underscore descends into a
section, e.g. <PREFIX>SECTION__KEY. A few short legacy names are still mapped.
"""

from relay import APP_NAME

PREFIX = APP_NAME.upper() + "_"
NESTING = "__"

# Old variable names -> setting key (kept for compatibility).
LEGACY = {
    "TIMEOUT": "timeout",
    "DEBUG": "verbose",
}

IGNORED = {"CONFIG"}


def read(environ):
    out = {}
    for name, value in environ.items():
        if not name.startswith(PREFIX):
            continue
        key = name[len(PREFIX):]
        if key in IGNORED:
            continue
        if key in LEGACY:
            out[LEGACY[key]] = value
            continue
        path = key.lower().split(NESTING)
        node = out
        for part in path[:-1]:
            node = node.setdefault(part, {})
        node[path[-1]] = value
    return out
