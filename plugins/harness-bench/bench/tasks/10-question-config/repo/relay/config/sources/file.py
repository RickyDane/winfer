"""ini-style config file: top-level keys under [relay], nested ones under their section."""

import configparser
import os


def read(path):
    if not os.path.exists(path):
        return {}
    parser = configparser.ConfigParser()
    parser.read(path)
    out = {}
    for section in parser.sections():
        values = dict(parser[section])
        if section == "relay":
            out.update(values)
        else:
            out.setdefault(section, {}).update(values)
    return out
