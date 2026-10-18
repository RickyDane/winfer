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
if len(got) != len(set(got)):
    print("duplicate entries"); sys.exit(1)
if set(got) != set(exp):
    print("missing:", sorted(set(exp) - set(got)), "extra:", sorted(set(got) - set(exp))); sys.exit(1)
PY
other=$(git status --porcelain --untracked-files=all | grep -v -e ' ANSWER.json$' -e '__pycache__/' -e '\.pyc$')
[ -z "$other" ] || { echo "other files changed:"; echo "$other"; exit 1; }
