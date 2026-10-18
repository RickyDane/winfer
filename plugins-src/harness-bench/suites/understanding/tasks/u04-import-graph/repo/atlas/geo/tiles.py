from . import projection
from .shapes import Shape


class TileGrid:
    def __init__(self, zoom):
        self.zoom = zoom
        self.mercator = projection.mercator
