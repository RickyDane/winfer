"""Compares legacy/invoices.py against its committed version:
- every original top-level name still exists with the same signature (summaryReport may gain `today=None`),
- every removed line lies inside summaryReport's original body,
- overdueInvoices exists."""
import ast
import re
import subprocess
import sys

PATH = "legacy/invoices.py"
old_src = subprocess.run(["git", "show", f"HEAD:{PATH}"], capture_output=True, text=True, check=True).stdout
new_src = open(PATH).read()
old, new = ast.parse(old_src), ast.parse(new_src)


def sigs(tree):
    out = {}
    for n in tree.body:
        if isinstance(n, ast.FunctionDef):
            out[n.name] = n
        elif isinstance(n, ast.Assign):
            for t in n.targets:
                if isinstance(t, ast.Name):
                    out[t.id] = n
    return out


o, n = sigs(old), sigs(new)
errors = []
for name, node in o.items():
    if name not in n:
        errors.append(f"{name} was removed or renamed")
        continue
    if isinstance(node, ast.FunctionDef):
        a, b = ast.dump(node.args), ast.dump(n[name].args)
        if name == "summaryReport":
            args = n[name].args
            names = [x.arg for x in args.args]
            ok = names == ["invoices", "today"] and len(args.defaults) == 1 \
                and isinstance(args.defaults[0], ast.Constant) and args.defaults[0].value is None \
                and not args.vararg and not args.kwarg and not args.kwonlyargs \
                and all(x.annotation is None for x in args.args) and n[name].returns is None
            if not ok:
                errors.append("summaryReport signature must be summaryReport(invoices, today=None)")
        elif a != b or ast.dump(node.returns or ast.Pass()) != ast.dump(n[name].returns or ast.Pass()):
            errors.append(f"signature of {name} changed")
    elif ast.dump(node.value) != ast.dump(n[name].value):
        errors.append(f"value of {name} changed")
if "overdueInvoices" not in n or not isinstance(n["overdueInvoices"], ast.FunctionDef):
    errors.append("overdueInvoices is missing")
else:
    if [x.arg for x in n["overdueInvoices"].args.args] != ["invoices", "today"]:
        errors.append("overdueInvoices must take (invoices, today)")

# Changed/removed lines must be inside summaryReport's original body.
sr = o["summaryReport"]
lo, hi = sr.lineno, sr.end_lineno
diff = subprocess.run(["git", "diff", "-U0", "HEAD", "--", PATH], capture_output=True, text=True, check=True).stdout
outside = []
for m in re.finditer(r"^@@ -(\d+)(?:,(\d+))? ", diff, re.M):
    start, count = int(m.group(1)), int(m.group(2) if m.group(2) is not None else 1)
    for ln in range(start, start + count):
        # Blank lines may shift when a function is inserted next to summaryReport.
        if not lo <= ln <= hi and old_src.splitlines()[ln - 1].strip():
            outside.append(ln)
if outside:
    lines = old_src.splitlines()
    shown = "; ".join(f"{ln}: {lines[ln-1].strip()}" for ln in outside[:3])
    errors.append(f"{len(outside)} existing line(s) outside summaryReport changed or removed ({shown})")

if errors:
    print("\n".join(errors))
    sys.exit(1)
