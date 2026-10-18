"""Attribute access over the merged settings dict."""


class Settings:
    def __init__(self, data):
        self._data = data

    def __getattr__(self, name):
        try:
            value = self._data[name]
        except KeyError:
            raise AttributeError(name) from None
        return Settings(value) if isinstance(value, dict) else value

    def as_dict(self):
        return self._data
