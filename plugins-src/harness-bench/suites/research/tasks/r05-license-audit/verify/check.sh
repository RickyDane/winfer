#!/bin/sh
[ -f ANSWER.json ] || { echo "no ANSWER.json"; exit 1; }
other=$(git status --porcelain | grep -v ' ANSWER.json$')
[ -z "$other" ] || { echo "other files changed:"; echo "$other"; exit 1; }
python3 -I "$VERIFY_DIR/check.py"
