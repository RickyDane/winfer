#!/bin/sh
fail() { rm -f bench_hidden_test.py; echo "$1"; exit 1; }
git diff --quiet HEAD -- tests || fail "tests/ was modified"
python3 -m unittest discover -s tests -q 2>&1 || fail "visible tests fail"
cp "$VERIFY_DIR/hidden_test.py" bench_hidden_test.py
out=$(python3 -m unittest -q bench_hidden_test 2>&1) || fail "$(echo "$out" | grep -E '^(FAIL|ERROR|AssertionError)' | head -8)"
rm -f bench_hidden_test.py
