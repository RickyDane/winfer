#!/bin/sh
cp "$VERIFY_DIR/test_hidden.py" bench_hidden_test.py
python3 -m unittest -q bench_hidden_test 2>&1; code=$?
rm -f bench_hidden_test.py
exit $code
