from .base import Store


class RedisStore(Store):
    def __init__(self, settings):
        super().__init__(settings)
        import redis

        self._r = redis.Redis.from_url(settings.get("storage.url", "redis://localhost:6379/0"))

    def put(self, key, value):
        self._r.set(key, value)

    def get(self, key):
        return self._r.get(key)
