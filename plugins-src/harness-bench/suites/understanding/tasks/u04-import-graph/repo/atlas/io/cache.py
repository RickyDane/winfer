try:
    from atlas.geo.projection import mercator as _m
except AttributeError:
    _m = None

CACHE = {}
