"""Projections (needs numpy + pyproj)."""

import numpy as np
import pyproj


class Projection:
    def __init__(self, crs):
        self.crs = pyproj.CRS(crs)


def mercator(lon, lat):
    return np.radians(lon), np.log(np.tan(np.pi / 4 + np.radians(lat) / 2))
