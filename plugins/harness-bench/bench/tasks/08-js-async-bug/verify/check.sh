#!/bin/sh
git diff --quiet HEAD -- test || { echo "test/ was modified"; exit 1; }
out=$(timeout 60 node --test 2>&1) || { echo "visible tests fail"; echo "$out" | grep -E "^not ok|✖|Error" | head -10; exit 1; }
mkdir -p .bench && cp "$VERIFY_DIR/hidden.test.mjs" .bench/hidden.test.mjs
out=$(timeout 60 node --test .bench/hidden.test.mjs 2>&1); code=$?
rm -rf .bench
[ $code -eq 0 ] || echo "$out" | grep -E "^not ok|✖|Error|expected|actual" | head -20
exit $code
