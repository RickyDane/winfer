#!/bin/sh
fail() { echo "$1"; exit 1; }
git diff --quiet HEAD -- tests || fail "tests/ was modified"
python3 -m unittest discover -s tests -q 2>&1 || fail "tests fail"
changed=$(git status --porcelain --untracked-files=all | grep -v -e '__pycache__/' -e '\.pyc$' | awk '{print $2}')
[ "$changed" = "ledger/currencies/nok.py" ] || fail "expected only ledger/currencies/nok.py to change, got: $(echo $changed)"
lines=$(git diff --numstat HEAD -- ledger/currencies/nok.py | awk '{print $1 + $2}')
[ "$lines" -le 4 ] || fail "diff too large: $lines changed lines"
