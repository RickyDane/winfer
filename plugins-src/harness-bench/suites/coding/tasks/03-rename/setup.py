"""Generates the project for this task into the directory given as argv[1] (deterministic)."""

import os
import random
import sys

root = sys.argv[1]
rng = random.Random(3)


def write(path, text):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text)


areas = ["billing", "orders", "catalog", "users", "shipping", "reports", "search", "notify"]
write("app/__init__.py", "")
for a in areas:
    write(f"app/{a}/__init__.py", "")

write(
    "app/billing/tax.py",
    '''"""Tax rules."""

RATES = {"DE": 0.19, "FR": 0.20, "US": 0.0, "UK": 0.20}


def compute_tax(cents: int, country: str) -> int:
    """Tax in cents for a net amount, rounded half up."""
    rate = RATES.get(country, 0.19)
    return int(cents * rate + 0.5)


def compute_tax_free(cents: int) -> int:
    """Amount for tax-free customers (unchanged); not the same as compute_tax."""
    return cents
''',
)

users = []
for i in range(36):
    area = areas[i % len(areas)]
    name = f"module_{i:02d}"
    uses = i % 4 == 1
    lines = ['"""Generated module."""', ""]
    if uses:
        lines.append("from app.billing.tax import compute_tax")
        users.append((area, name))
    if i % 9 == 2:
        lines.append("from app.billing.tax import compute_tax_free")
    lines += ["", ""]
    for j in range(rng.randint(3, 7)):
        k = rng.randint(2, 9)
        lines.append(f"def helper_{j}(x):")
        lines.append(f"    return x * {k} + {j}")
        lines.append("")
        lines.append("")
    if uses:
        lines.append("def gross(cents, country):")
        lines.append("    # compute_tax rounds half up")
        lines.append("    return cents + compute_tax(cents, country)")
        lines.append("")
    if i % 9 == 2:
        lines.append("def exempt(cents):")
        lines.append("    return compute_tax_free(cents)")
        lines.append("")
    write(f"app/{area}/{name}.py", "\n".join(lines))

write(
    "docs/billing.md",
    "# Billing\n\nUse `compute_tax(cents, country)` from `app.billing.tax` for tax.\n"
    "Tax-free customers use `compute_tax_free(cents)`.\n",
)

tests = ["import unittest", "", "from app.billing import tax"]
for area, name in users:
    tests.append(f"from app.{area} import {name}")
tests += ["", "", "class TaxTest(unittest.TestCase):", "    def test_call_sites(self):"]
for area, name in users:
    tests.append(f"        self.assertEqual({name}.gross(1000, 'DE'), 1190)")
tests += ["", "    def test_free(self):", "        self.assertEqual(tax.compute_tax_free(500), 500)", "", "", 'if __name__ == "__main__":', "    unittest.main()", ""]
write("tests/test_tax.py", "\n".join(tests))
