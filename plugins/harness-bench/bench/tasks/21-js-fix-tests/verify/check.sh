#!/bin/sh
git diff --quiet HEAD -- test || { echo "test/ was modified"; exit 1; }
out=$(node --test 2>&1) || { echo "$out" | grep -E "^✖|^not ok" | head -10; echo "visible tests fail"; exit 1; }
mkdir -p .bench && cp "$VERIFY_DIR/hidden-tests.mjs" .bench/hidden.test.mjs
out=$(node --test .bench/hidden.test.mjs 2>&1); code=$?
rm -rf .bench
[ $code -eq 0 ] || echo "$out" | grep -E "^not ok|✖|Error|expected|actual" | head -20
exit $code
