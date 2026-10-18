#!/bin/sh
fail() { echo "$1"; exit 1; }
ignore='__pycache__|\.pyc$|\.pytest_cache|\.bench-output'
# Only inventory/stock.py may change: at most 2 lines, all inside the body of low_stock.
other=$(git status --porcelain --untracked-files=all | grep -vE "$ignore" | grep -v ' inventory/stock.py$')
[ -z "$other" ] || { echo "$other" | head -5; fail "files other than inventory/stock.py changed"; }
set -- $(git diff --numstat HEAD -- inventory/stock.py)
added=${1:-0}; removed=${2:-0}
[ "$added" -ge 1 ] || fail "inventory/stock.py was not changed"
[ "$added" -le 2 ] && [ "$removed" -le 2 ] || fail "diff too large: +$added -$removed lines in inventory/stock.py (expected a one-line fix)"
python3 - <<'PY' || exit 1
import ast, re, subprocess, sys
old = subprocess.run(["git", "show", "HEAD:inventory/stock.py"], capture_output=True, text=True, check=True).stdout
fn = next(n for n in ast.parse(old).body if isinstance(n, ast.FunctionDef) and n.name == "low_stock")
lo, hi = fn.body[1].lineno, fn.end_lineno  # the code below the docstring
diff = subprocess.run(["git", "diff", "-U0", "HEAD", "--", "inventory/stock.py"], capture_output=True, text=True, check=True).stdout
for m in re.finditer(r"^@@ -(\d+)(?:,(\d+))? ", diff, re.M):
    start, count = int(m.group(1)), int(m.group(2) if m.group(2) is not None else 1)
    for ln in range(start, start + max(count, 1)):
        if not lo <= ln <= hi:
            print(f"line {ln} outside the body of low_stock changed: {old.splitlines()[ln-1].strip()[:60]!r}")
            sys.exit(1)
# No quote or literal changes on the touched lines: same string literals before and after.
import io, tokenize
def strings(lines):
    toks = tokenize.generate_tokens(io.StringIO("\n".join(l.strip() for l in lines) + "\n").readline)
    try:
        return sorted(t.string for t in toks if t.type == tokenize.STRING)
    except (tokenize.TokenError, SyntaxError):
        return None
minus = [l[1:] for l in diff.splitlines() if l.startswith("-") and not l.startswith("---")]
plus = [l[1:] for l in diff.splitlines() if l.startswith("+") and not l.startswith("+++")]
if strings(minus) != strings(plus):
    print(f"string literals/quotes changed on the touched lines: {strings(minus)} -> {strings(plus)}")
    sys.exit(1)
PY
python3 -m unittest discover -s tests -q 2>&1 || fail "visible tests fail"
cp "$VERIFY_DIR/test_hidden.py" bench_hidden_test.py
python3 -m unittest -q bench_hidden_test 2>&1; code=$?
rm -f bench_hidden_test.py
[ $code -eq 0 ] || fail "hidden tests fail"
