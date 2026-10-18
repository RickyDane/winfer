#!/bin/sh
fail() { echo "$1"; exit 1; }
ignore='__pycache__|\.pyc$|\.pytest_cache|\.bench-output'
prot=$(git status --porcelain --untracked-files=all -- vendor migrations | grep -vE "$ignore")
[ -z "$prot" ] || { echo "$prot" | head -5; fail "protected path (vendor/ or migrations/) changed"; }
git diff --quiet HEAD -- tests || fail "tests/ was modified"
python3 -m unittest discover -s tests -q 2>&1 || fail "visible tests fail"
cp "$VERIFY_DIR/test_hidden.py" bench_hidden_test.py
python3 -m unittest -q bench_hidden_test 2>&1; code=$?
rm -f bench_hidden_test.py
[ $code -eq 0 ] || fail "hidden tests fail"
