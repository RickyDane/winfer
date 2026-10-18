import configparser
import os

from .defaults import DEFAULTS


class Settings:
    def __init__(self, values):
        self._values = values

    def get(self, key, default=None):
        return self._values.get(key, default)


def load(path=None):
    path = path or os.environ.get("COURIER_CONFIG", "courier.ini")
    values = dict(DEFAULTS)
    parser = configparser.ConfigParser()
    parser.read(path)  # a missing file is silently ignored
    for section in parser.sections():
        for key, value in parser[section].items():
            values[f"{section}.{key}"] = value
    return Settings(values)
