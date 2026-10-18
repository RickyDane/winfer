#!/bin/sh
fail() { rm -f bench_hidden_perf.py; echo "$1"; exit 1; }
git diff --quiet HEAD -- tests || fail "tests/ was modified"
python3 -m unittest discover -s tests -q 2>&1 || fail "visible tests fail"
cp "$VERIFY_DIR/hidden_perf.py" bench_hidden_perf.py
out=$(timeout 30 python3 -m unittest -q bench_hidden_perf 2>&1); code=$?
[ $code -eq 124 ] && fail "hidden tests timed out (still too slow)"
[ $code -eq 0 ] || fail "$(echo "$out" | tail -15)"
rm -f bench_hidden_perf.py
