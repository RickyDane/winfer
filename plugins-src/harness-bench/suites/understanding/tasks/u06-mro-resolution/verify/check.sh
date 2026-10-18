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
for k in ("handler_class", "first_handle"):
    if not isinstance(got[k], str) or got[k].strip().split(".")[-1] != exp[k]:
        print("wrong", k); sys.exit(1)
if type(got["base_calls"]) is not int or got["base_calls"] != exp["base_calls"]:
    print("wrong base_calls"); sys.exit(1)
if got["result"] != exp["result"]:
    print("wrong result"); sys.exit(1)
PY
other=$(git status --porcelain --untracked-files=all | grep -v -e ' ANSWER.json$' -e '__pycache__/' -e '\.pyc$')
[ -z "$other" ] || { echo "other files changed:"; echo "$other"; exit 1; }
