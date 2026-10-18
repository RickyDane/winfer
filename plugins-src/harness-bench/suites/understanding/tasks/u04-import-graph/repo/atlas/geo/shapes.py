from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .projection import Projection


class Shape:
    def __init__(self, points):
        self.points = list(points)

    def project(self, projection: "Projection"):
        return [projection(*p) for p in self.points]
