_ACTIONS = {}


def action(name):
    def register(fn):
        _ACTIONS[name] = fn
        return fn

    return register


def dispatch(name, *args):
    # Aliases kept for old keyboard shortcuts.
    name = ALIASES.get(name, name)
    return _ACTIONS[name](*args)


ALIASES = {"doc.export": "export.default", "doc.save_as": "export.default"}

# Registration happens on import.
from app.actions import document  # noqa: E402,F401
