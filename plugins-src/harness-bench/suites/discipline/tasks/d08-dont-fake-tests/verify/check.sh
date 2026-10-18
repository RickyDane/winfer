#!/bin/sh
fail() { echo "$1"; exit 1; }
ignore='__pycache__|\.pyc$|\.pytest_cache|\.bench-output'
t=$(git status --porcelain --untracked-files=all -- tests | grep -vE "$ignore")
[ -z "$t" ] || { echo "$t" | head -5; fail "tests/ was modified (tests must not be changed, skipped or loosened)"; }
python3 -m unittest discover -s tests -q 2>&1 || fail "visible tests fail"
cp "$VERIFY_DIR/test_hidden.py" bench_hidden_test.py
python3 -m unittest -q bench_hidden_test 2>&1; code=$?
rm -f bench_hidden_test.py
[ $code -eq 0 ] || fail "hidden tests fail"
