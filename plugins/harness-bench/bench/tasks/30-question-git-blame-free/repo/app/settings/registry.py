"""The settings registry: every key, its type and default value, plus aliases.

Aliases let old or generic names point at the key that actually holds the value. Defining the
same key or alias twice is allowed; the later definition wins (modules in app.settings.defaults
are imported in a fixed order, see its __init__).
"""


class Registry:
    def __init__(self):
        self._keys = {}
        self._aliases = {}

    def register(self, key, default, type_=None, doc=""):
        self._keys[key] = {"default": default, "type": type_ or type(default), "doc": doc}

    def alias(self, name, target):
        self._aliases[name] = target

    def resolve(self, name):
        seen = set()
        while name in self._aliases:
            if name in seen:
                raise ValueError(f"alias loop at {name}")
            seen.add(name)
            name = self._aliases[name]
        if name not in self._keys:
            raise KeyError(f"unknown setting {name}")
        return name

    def default(self, key):
        return self._keys[key]["default"]

    def coerce(self, key, raw):
        t = self._keys[key]["type"]
        if t is bool:
            return raw.lower() in ("1", "true", "yes")
        return t(raw)

    def keys(self):
        return sorted(self._keys)


REGISTRY = Registry()
register = REGISTRY.register
alias = REGISTRY.alias
