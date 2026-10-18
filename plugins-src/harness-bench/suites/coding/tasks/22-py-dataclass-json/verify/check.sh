#!/bin/sh
# Visible tests must pass unchanged, plus the hidden ones.
set -e
git diff --quiet HEAD -- tests/test_serde.py || { echo "tests/test_serde.py was modified"; exit 1; }
python3 -m unittest discover -s tests -q
cp "$VERIFY_DIR/test_hidden.py" bench_hidden_test.py
python3 -m unittest -q bench_hidden_test 2>&1 || { rm -f bench_hidden_test.py; exit 1; }
rm -f bench_hidden_test.py
