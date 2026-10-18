#!/bin/sh
[ -f ANSWER.txt ] || { echo "no ANSWER.txt"; exit 1; }
python3 -I - "$VERIFY_DIR/expected.txt" ANSWER.txt <<'PY' || exit 1
import sys
def norm(p):
    lines = [l.rstrip() for l in open(p, encoding="utf-8", errors="replace").read().splitlines()]
    lines = [l for l in lines if not l.startswith("```")]
    while lines and not lines[0]:
        lines.pop(0)
    while lines and not lines[-1]:
        lines.pop()
    return lines
exp, got = norm(sys.argv[1]), norm(sys.argv[2])
if exp != got:
    for i, (e, g) in enumerate(zip(exp + [""] * 9, got + [""] * 9)):
        if e != g:
            print(f"line {i + 1} differs: {g!r}")
            break
    sys.exit(1)
PY
other=$(git status --porcelain --untracked-files=all | grep -v -e ' ANSWER.txt$' -e '__pycache__/' -e '\.pyc$')
[ -z "$other" ] || { echo "other files changed:"; echo "$other"; exit 1; }
