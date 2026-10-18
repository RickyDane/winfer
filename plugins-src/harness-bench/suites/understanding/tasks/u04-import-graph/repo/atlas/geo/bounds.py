from .shapes import Shape


def bbox(shape: Shape):
    xs = [p[0] for p in shape.points]
    ys = [p[1] for p in shape.points]
    return min(xs), min(ys), max(xs), max(ys)


def projected_bbox(shape):
    from .projection import mercator

    return bbox(Shape(mercator(*p) for p in shape.points))
