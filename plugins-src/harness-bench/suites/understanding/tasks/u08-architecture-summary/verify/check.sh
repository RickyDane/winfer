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
bad = []
s = lambda v: v.strip() if isinstance(v, str) else v
if s(got["entrypoint"]) != exp["entrypoint"]: bad.append("entrypoint")
if s(got["storage_backend"]) != exp["storage_backend"]: bad.append("storage_backend")
if type(got["max_attempts"]) is not int or got["max_attempts"] != exp["max_attempts"]: bad.append("max_attempts")
h = got["handlers"]
if not isinstance(h, list) or sorted(map(s, h)) != exp["handlers"] or len(h) != len(exp["handlers"]): bad.append("handlers")
m = got["middleware_order"]
if not isinstance(m, list) or [s(x) for x in m] != exp["middleware_order"]: bad.append("middleware_order")
if bad:
    print("wrong:", ", ".join(bad)); sys.exit(1)
PY
other=$(git status --porcelain --untracked-files=all | grep -v -e ' ANSWER.json$' -e '__pycache__/' -e '\.pyc$')
[ -z "$other" ] || { echo "other files changed:"; echo "$other"; exit 1; }
