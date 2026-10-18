import numpy as np

import atlas.geo
from atlas.geo import mercator


def mean_point(points):
    return tuple(np.mean(np.array(points), axis=0))
