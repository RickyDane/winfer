#!/bin/sh
cp "$VERIFY_DIR/test_hidden.py" bench_hidden_test.py
out=$(python3 -m unittest -q bench_hidden_test 2>&1); code=$?
rm -rf bench_hidden_test.py __pycache__
[ $code -eq 0 ] || echo "$out" | tail -25
exit $code
