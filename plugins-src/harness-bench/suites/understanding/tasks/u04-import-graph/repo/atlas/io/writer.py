import json

from . import formats


def write(path, data):
    with open(path, "w") as f:
        json.dump(data, f)


def write_projected(path, data):
    from . import loader

    write(path, loader.load(data))
