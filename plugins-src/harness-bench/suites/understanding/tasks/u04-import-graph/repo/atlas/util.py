def clamp(v, lo, hi):
    return max(lo, min(v, hi))


def pairwise(seq):
    return list(zip(seq, seq[1:]))
