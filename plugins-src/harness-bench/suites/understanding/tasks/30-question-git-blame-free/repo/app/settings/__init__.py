"""Settings: registered defaults, overridden by environment variables, then by explicit overrides."""

import os

from app.settings import defaults  # noqa: F401  (registers every key)
from app.settings.registry import REGISTRY


class Settings:
    def __init__(self, overrides=None):
        self._overrides = {REGISTRY.resolve(k): v for k, v in (overrides or {}).items()}

    def get(self, name):
        key = REGISTRY.resolve(name)
        if key in self._overrides:
            return self._overrides[key]
        env = "SYNC_" + key.upper().replace(".", "_")
        if env in os.environ:
            return REGISTRY.coerce(key, os.environ[env])
        return REGISTRY.default(key)

    def get_int(self, name):
        return int(self.get(name))

    def get_float(self, name):
        return float(self.get(name))

    def get_bool(self, name):
        value = self.get(name)
        return value if isinstance(value, bool) else str(value).lower() in ("1", "true", "yes")
