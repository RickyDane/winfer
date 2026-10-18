PLUGINS = {}


def register(name):
    def deco(fn):
        PLUGINS[name] = fn
        return fn

    return deco


def _load_builtin_tiles():
    from atlas.geo import tiles

    PLUGINS["tiles"] = tiles.TileGrid


_load_builtin_tiles()
