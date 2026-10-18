from atlas.geo.shapes import Shape


def shoelace(shape: Shape):
    pts = shape.points
    return abs(sum(x1 * y2 - x2 * y1 for (x1, y1), (x2, y2) in zip(pts, pts[1:] + pts[:1]))) / 2
