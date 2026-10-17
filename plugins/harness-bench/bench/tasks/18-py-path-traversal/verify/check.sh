#!/bin/sh
# Visible tests must pass unchanged, plus the hidden ones.
git diff --quiet HEAD -- tests || { echo "tests/ was modified"; exit 1; }
python3 -m unittest discover -s tests -q 2>&1 | tail -5
python3 -m unittest discover -s tests -q >/dev/null 2>&1 || { echo "visible tests fail"; exit 1; }
cp "$VERIFY_DIR/test_hidden.py" bench_hidden_test.py
out=$(python3 -m unittest -q bench_hidden_test 2>&1); code=$?
rm -f bench_hidden_test.py
[ $code -eq 0 ] || echo "$out" | tail -25
exit $code
