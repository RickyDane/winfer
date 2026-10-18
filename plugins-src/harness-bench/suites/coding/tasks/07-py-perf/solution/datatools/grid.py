"""Grid path counting."""

MOD = 1_000_000_007


def count_paths(grid):
    """Number of paths from the top-left to the bottom-right cell, modulo 1_000_000_007.

    `grid` is a list of equal-length strings; "." is open and "#" is blocked. A path moves
    only right or down and visits open cells only. An empty grid, or a blocked start or
    end cell, has 0 paths; a single open cell has 1.
    """
    if not grid or not grid[0]:
        return 0
    w = len(grid[0])
    row = [0] * w
    row[0] = 1
    for line in grid:
        for c in range(w):
            if line[c] == "#":
                row[c] = 0
            elif c:
                row[c] = (row[c] + row[c - 1]) % MOD
    return row[-1] % MOD
