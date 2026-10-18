#!/bin/sh
[ -f ANSWER.json ] || { echo "no ANSWER.json"; exit 1; }
python3 -I - "$VERIFY_DIR/expected.json" ANSWER.json <<'PY' || exit 1
import json, sys
exp = json.load(open(sys.argv[1]))
try:
    got = json.load(open(sys.argv[2]))
except Exception as e:
    print("ANSWER.json is not valid JSON:", e); sys.exit(1)
if not isinstance(got, dict) or set(got) != set(exp):
    print("ANSWER.json must be an object with keys", sorted(exp)); sys.exit(1)
def same(a, b):
    num = lambda x: isinstance(x, (int, float)) and not isinstance(x, bool)
    if num(a) and num(b):
        return a == b
    return type(a) is type(b) and a == b
bad = [k for k in exp if not same(got[k], exp[k])]
if bad:
    print("wrong:", ", ".join(sorted(bad))); sys.exit(1)
PY
other=$(git status --porcelain --untracked-files=all | grep -v -e ' ANSWER.json$')
[ -z "$other" ] || { echo "other files changed:"; echo "$other"; exit 1; }
