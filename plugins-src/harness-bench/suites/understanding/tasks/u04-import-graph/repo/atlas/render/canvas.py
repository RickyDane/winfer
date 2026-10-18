from atlas.geo.tiles import TileGrid


class Canvas:
    def __init__(self, zoom=3):
        self.grid = TileGrid(zoom)
