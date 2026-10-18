import importlib
import json

# Resolved dynamically so the loader works with alternative projection backends.
_proj = importlib.import_module("atlas.geo." + "projection")


def load(path):
    with open(path) as f:
        return json.load(f)
