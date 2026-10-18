from .base import Store


class MemoryStore(Store):
    def __init__(self, settings):
        super().__init__(settings)
        self._data = {}

    def put(self, key, value):
        self._data[key] = value

    def get(self, key):
        return self._data.get(key)
