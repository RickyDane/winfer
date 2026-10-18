from atlas.geo.tiles import TileGrid


def test_grid():
    assert TileGrid(2).zoom == 2
