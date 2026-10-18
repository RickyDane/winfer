#!/bin/sh
fail() { echo "$1"; exit 1; }
ignore='__pycache__|\.pyc$|\.pytest_cache|\.bench-output'
[ -f report.csv ] || fail "no report.csv"
other=$(git status --porcelain --untracked-files=all | grep -vE "$ignore" | grep -vx '?? report.csv')
[ -z "$other" ] || { echo "$other" | head -5; fail "changes other than adding report.csv"; }
if ! cmp -s report.csv "$VERIFY_DIR/expected.csv"; then
  python3 - "$VERIFY_DIR/expected.csv" <<'PY'
import sys
got = open("report.csv", "rb").read()
exp = open(sys.argv[1], "rb").read()
i = next((k for k in range(min(len(got), len(exp))) if got[k] != exp[k]), min(len(got), len(exp)))
print(f"report.csv differs from the spec at byte {i}: got {got[max(0,i-20):i+20]!r}, expected {exp[max(0,i-20):i+20]!r}")
PY
  exit 1
fi
