#!/bin/sh
fail() { rm -f bench_hidden_test.py; echo "$1"; exit 1; }
git diff --quiet HEAD -- tests || fail "tests/ was modified"
run() {
  # Deadlocks show up as a timeout.
  if command -v timeout >/dev/null 2>&1; then timeout "$@"; else perl -e '$t = shift; alarm $t; exec @ARGV' "$@"; fi
}
out=$(run 30 python3 -m unittest discover -s tests -q 2>&1) || fail "visible tests fail or hang: $(echo "$out" | tail -3)"
cp "$VERIFY_DIR/hidden_test.py" bench_hidden_test.py
out=$(run 60 python3 -m unittest -q bench_hidden_test 2>&1); code=$?
[ $code -eq 0 ] || fail "hidden stress tests failed (exit $code; 124/142 = hung): $(echo "$out" | grep -E '^(FAIL|ERROR|AssertionError)' | head -5)"
rm -f bench_hidden_test.py
