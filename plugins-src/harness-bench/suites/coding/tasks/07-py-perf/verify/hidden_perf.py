import random
import time
import unittest

from datatools.contacts import dedupe_contacts
from datatools.grid import count_paths

MOD = 1_000_000_007


def ref_paths(grid):
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
    return row[-1]


def ref_dedupe(contacts):
    result, seen = [], {}
    for c in contacts:
        key = (c.get("email") or "").strip().lower()
        if not key:
            result.append(dict(c))
            continue
        if key not in seen:
            seen[key] = dict(c)
            result.append(seen[key])
            continue
        k = seen[key]
        for f, v in c.items():
            if k.get(f) in (None, "") and v not in (None, ""):
                k[f] = v
    return result


class Correctness(unittest.TestCase):
    def test_grid_edges(self):
        self.assertEqual(count_paths([]), 0)
        self.assertEqual(count_paths([""]), 0)
        self.assertEqual(count_paths(["."]), 1)
        self.assertEqual(count_paths(["#"]), 0)
        self.assertEqual(count_paths(["#.", ".."]), 0)
        self.assertEqual(count_paths(["..", ".#"]), 0)
        self.assertEqual(count_paths(["....."]), 1)
        self.assertEqual(count_paths(["."] * 5), 1)
        self.assertEqual(count_paths([".#.", "...", "#.."]), 2)

    def test_grid_random_small(self):
        rng = random.Random(7)
        for _ in range(200):
            h, w = rng.randint(1, 9), rng.randint(1, 9)
            g = ["".join("#" if rng.random() < 0.2 else "." for _ in range(w)) for _ in range(h)]
            self.assertEqual(count_paths(g), ref_paths(g), g)

    def test_grid_modulo(self):
        g = ["." * 40] * 40  # C(78, 39) > MOD
        self.assertEqual(count_paths(g), ref_paths(g))
        self.assertLess(count_paths(g), MOD)

    def test_dedupe_semantics(self):
        a = {"email": "X@a.io", "name": "", "city": None}
        b = {"email": "x@a.io", "name": "Xa", "city": "Rome", "zip": "1"}
        n1 = {"email": "", "name": "anon"}
        n2 = {"name": "anon"}
        c = {"email": "x@A.io  ", "name": "Other", "city": "Oslo", "note": ""}
        inp = [n1, a, n2, b, {"email": "y@a.io"}, c, {"email": None, "name": "n"}]
        out = dedupe_contacts(inp)
        self.assertEqual(out, [
            {"email": "", "name": "anon"},
            {"email": "X@a.io", "name": "Xa", "city": "Rome", "zip": "1"},
            {"name": "anon"},
            {"email": "y@a.io"},
            {"email": None, "name": "n"},
        ])
        self.assertEqual(a, {"email": "X@a.io", "name": "", "city": None})
        self.assertEqual(b["name"], "Xa")
        self.assertIsNot(out[0], n1)

    def test_dedupe_random(self):
        rng = random.Random(3)
        emails = [f"u{i}@ex.com" for i in range(30)] + ["", None]
        inp = []
        for _ in range(500):
            e = rng.choice(emails)
            if e and rng.random() < 0.3:
                e = " " + e.upper()
            d = {"email": e}
            for f in ("name", "phone", "city"):
                if rng.random() < 0.6:
                    d[f] = rng.choice(["", None, f"{f}{rng.randint(0, 9)}"])
            inp.append(d)
        self.assertEqual(dedupe_contacts(inp), ref_dedupe(inp))


class Speed(unittest.TestCase):
    def test_grid_large(self):
        rng = random.Random(1)
        g = ["".join("#" if rng.random() < 0.05 else "." for _ in range(600)) for _ in range(600)]
        g[0] = "." + g[0][1:]
        g[-1] = g[-1][:-1] + "."
        t = time.time()
        got = count_paths(g)
        self.assertEqual(got, ref_paths(g))
        self.assertLess(time.time() - t, 4)

    def test_dedupe_large(self):
        rng = random.Random(2)
        inp = [{"email": f"user{rng.randint(0, 80000)}@Ex.com", "name": rng.choice(["", "n"]), "i": i} for i in range(200000)]
        t = time.time()
        got = dedupe_contacts(inp)
        self.assertEqual(got, ref_dedupe(inp))
        self.assertLess(time.time() - t, 4)


if __name__ == "__main__":
    unittest.main()
