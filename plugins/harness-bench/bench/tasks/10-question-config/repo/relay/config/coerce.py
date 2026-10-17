"""Turns strings from files, env and flags into the type of the default."""


def coerce(value, like):
    if not isinstance(value, str) or like is None:
        return value
    if isinstance(like, bool):
        return value.lower() in ("1", "true", "yes", "on")
    if isinstance(like, int):
        return int(value)
    if isinstance(like, float):
        return float(value)
    return value


def coerce_tree(tree, defaults):
    out = {}
    for key, value in tree.items():
        like = defaults.get(key) if isinstance(defaults, dict) else None
        if isinstance(value, dict):
            out[key] = coerce_tree(value, like or {})
        else:
            out[key] = coerce(value, like)
    return out
