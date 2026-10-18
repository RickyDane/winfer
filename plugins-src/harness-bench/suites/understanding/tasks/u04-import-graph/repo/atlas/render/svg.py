def path(points):
    return "M" + " L".join(f"{x},{y}" for x, y in points)
