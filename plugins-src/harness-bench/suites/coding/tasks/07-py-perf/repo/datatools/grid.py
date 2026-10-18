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
    h, w = len(grid), len(grid[0])

    def walk(r, c):
        if r >= h or c >= w or grid[r][c] == "#":
            return 0
        if r == h - 1 and c == w - 1:
            return 1
        return walk(r + 1, c) + walk(r, c + 1)

    return walk(0, 0) % MOD
