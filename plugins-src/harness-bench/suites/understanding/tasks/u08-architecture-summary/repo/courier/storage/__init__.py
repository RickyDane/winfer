from .memory import MemoryStore
from .redis_store import RedisStore
from .sqlite import SqliteStore

BACKENDS = {
    "memory": MemoryStore,
    "sqlite": SqliteStore,
    "redis": RedisStore,
}


def make_store(settings):
    name = settings.get("storage.backend", "memory").strip().lower()
    return BACKENDS[name](settings)


from . import sharded  # noqa: E402,F401  (may register extra backends)
