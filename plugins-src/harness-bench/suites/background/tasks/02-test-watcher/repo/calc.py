def average(xs):
    return sum(xs) / len(xs) if xs else 0


def clamp(x, lo, hi):
    return max(lo, min(x, lo))


def percent(part, whole):
    return round(100 * part / whole)
