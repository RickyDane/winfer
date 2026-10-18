from atlas.geo import bounds


def load():
    from atlas.geo.projection import Projection

    return Projection("EPSG:3857")


def bbox_plugin(shape):
    return bounds.bbox(shape)
