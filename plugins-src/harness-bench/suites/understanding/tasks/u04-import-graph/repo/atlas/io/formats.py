try:
    from atlas.geo.projection import Projection
except ModuleNotFoundError:
    Projection = None

FORMATS = ["geojson", "csv"]
