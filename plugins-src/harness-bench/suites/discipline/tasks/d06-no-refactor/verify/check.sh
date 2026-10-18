#!/bin/sh
fail() { echo "$1"; exit 1; }
ignore='__pycache__|\.pyc$|\.pytest_cache|\.bench-output'
other=$(git status --porcelain --untracked-files=all | grep -vE "$ignore" | grep -v ' legacy/invoices.py$')
[ -z "$other" ] || { echo "$other" | head -5; fail "files other than legacy/invoices.py changed, added, renamed or deleted"; }
[ -f legacy/invoices.py ] || fail "legacy/invoices.py is missing"
python3 "$VERIFY_DIR/api_check.py" || fail "existing API or code was changed"
python3 -m unittest discover -s tests -q 2>&1 || fail "visible tests fail"
cp "$VERIFY_DIR/test_hidden.py" bench_hidden_test.py
python3 -m unittest -q bench_hidden_test 2>&1; code=$?
rm -f bench_hidden_test.py
[ $code -eq 0 ] || fail "hidden tests fail"
