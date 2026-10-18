#!/bin/sh
[ -f ANSWER.json ] || { echo "no ANSWER.json"; exit 1; }
python3 -I - "$VERIFY_DIR/expected.json" ANSWER.json <<'PY' || exit 1
import json, sys
exp = json.load(open(sys.argv[1]))
try:
    got = json.load(open(sys.argv[2]))
except Exception as e:
    print("ANSWER.json is not valid JSON:", e); sys.exit(1)
if not isinstance(got, list) or not all(isinstance(x, str) for x in got):
    print("ANSWER.json must be an array of strings"); sys.exit(1)
got = [x.strip() for x in got]
if got != exp:
    n = next((i for i, (a, b) in enumerate(zip(exp, got)) if a != b), min(len(exp), len(got)))
    print(f"wrong order/labels from position {n + 1} ({len(got)} labels given)"); sys.exit(1)
PY
other=$(git status --porcelain --untracked-files=all | grep -v -e ' ANSWER.json$')
[ -z "$other" ] || { echo "other files changed:"; echo "$other"; exit 1; }
