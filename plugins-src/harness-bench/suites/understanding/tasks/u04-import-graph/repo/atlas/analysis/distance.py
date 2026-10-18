import sys

if sys.version_info >= (3, 8):
    import atlas.geo.projection as P
else:
    P = None


def great_circle(a, b):
    return P.mercator(*a), P.mercator(*b)
