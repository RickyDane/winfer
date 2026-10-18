import ast
import random
import unittest
from pathlib import Path

import bench_reference_reports as ref
import reports

PUBLIC = ["sales_report", "refund_report", "inventory_report"]
LOOPS = (ast.For, ast.AsyncFor, ast.While, ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)


def code_lines(src):
    return sum(1 for l in src.splitlines() if l.strip() and not l.strip().startswith("#"))


class Output(unittest.TestCase):
    def rows(self, rng):
        regions = ["north", "south", " east ", "", "   ", "west", "Zeta", "alpha", None]
        out = []
        for _ in range(rng.randrange(0, 40)):
            r = {}
            if rng.random() < 0.9:
                r["region"] = rng.choice(regions)
                r["warehouse"] = rng.choice(regions)
            r["status"] = rng.choice(["paid", "paid", "refunded", "pending", None])
            r["amount_cents"] = rng.choice([0, 1, 5, 99, 100, 12345, 999999, -250])
            if rng.random() < 0.9:
                r["qty"] = rng.choice([-3, 0, 0, 1, 2, 7, 100])
            r["sku"] = "S%d" % rng.randrange(10)
            out.append({k: v for k, v in r.items() if v is not None or k == "region"})
        return out

    def test_matches_original(self):
        rng = random.Random(7)
        for i in range(600):
            rows = self.rows(rng)
            for name in PUBLIC:
                args = (rows,) if i % 5 else (rows, "CUSTOM TITLE %d" % i)
                self.assertEqual(getattr(reports, name)(*args), getattr(ref, name)(*args), f"{name} differs on {rows!r}")

    def test_ties_and_empty(self):
        rows = [{"region": r, "warehouse": r, "status": s, "amount_cents": 100, "qty": 1} for r in "bacb" for s in ("paid", "refunded")]
        for name in PUBLIC:
            self.assertEqual(getattr(reports, name)(rows), getattr(ref, name)(rows))
            self.assertEqual(getattr(reports, name)([]), getattr(ref, name)([]))
            self.assertEqual(getattr(reports, name)([], title="X"), getattr(ref, name)([], title="X"))


class Structure(unittest.TestCase):
    def setUp(self):
        self.src = Path("reports.py").read_text()
        self.tree = ast.parse(self.src)
        self.funcs = {n.name: n for n in self.tree.body if isinstance(n, ast.FunctionDef)}

    def test_public_functions_have_no_loops(self):
        for name in PUBLIC:
            self.assertIn(name, self.funcs)
            loops = [n for n in ast.walk(self.funcs[name]) if isinstance(n, LOOPS)]
            self.assertFalse(loops, f"{name} still contains a loop")

    def test_single_shared_helper(self):
        called = []
        for name in PUBLIC:
            names = {n.func.id for n in ast.walk(self.funcs[name]) if isinstance(n, ast.Call) and isinstance(n.func, ast.Name)}
            called.append(names & {f for f in self.funcs if f not in PUBLIC})
        shared = set.intersection(*called)
        self.assertTrue(shared, "no private helper is called by all three public functions")
        helper_loops = [n for f in shared for n in ast.walk(self.funcs[f]) if isinstance(n, (ast.For, ast.While))]
        self.assertTrue(helper_loops, "the shared helper does not contain the grouping loop")
        total_for = sum(isinstance(n, (ast.For, ast.While)) for n in ast.walk(self.tree))
        self.assertLessEqual(total_for, 3, f"{total_for} for/while loops left in reports.py (duplicated loop bodies?)")

    def test_shorter(self):
        before = code_lines(Path("bench_reference_reports.py").read_text())
        after = code_lines(self.src)
        self.assertLessEqual(after, before * 0.8, f"reports.py only shrank from {before} to {after} code lines")


if __name__ == "__main__":
    unittest.main()
