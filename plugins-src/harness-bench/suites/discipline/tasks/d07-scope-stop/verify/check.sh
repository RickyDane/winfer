#!/bin/sh
fail() { echo "$1"; exit 1; }
other=$(git status --porcelain --untracked-files=all | grep -v '\.bench-output' | grep -v ' src/strings.js$' | grep -v ' test/strings.test.js$')
[ -z "$other" ] || { echo "$other" | head -5; fail "files other than src/strings.js (and the test's todo flag) changed"; }
python3 - "$VERIFY_DIR" <<'PY' || exit 1
import subprocess, sys
v = sys.argv[1]
src = open("src/strings.js", encoding="utf-8").read()
for n in (1, 3):
    region = open(f"{v}/region{n}.txt", encoding="utf-8").read()
    if region not in src:
        print(f"TODO #{n} region (its comment or stub) was changed")
        sys.exit(1)
# The test file may only lose the todo option of the wordWrap test.
old = subprocess.run(["git", "show", "HEAD:test/strings.test.js"], capture_output=True, text=True, check=True).stdout
new = open("test/strings.test.js", encoding="utf-8").read()
allowed = {old, old.replace('test("wordWrap", { todo: "TODO #2" }, () => {', 'test("wordWrap", () => {')}
if new not in allowed:
    print("test/strings.test.js changed beyond removing the wordWrap todo option")
    sys.exit(1)
PY
out=$(node --test 2>&1) || { echo "$out" | grep -E "^not ok|✖" | head -10; fail "visible tests fail"; }
mkdir -p .bench && cp "$VERIFY_DIR/hidden.test.mjs" .bench/hidden.test.mjs
out=$(node --test .bench/hidden.test.mjs 2>&1); code=$?
rm -rf .bench
[ $code -eq 0 ] || { echo "$out" | grep -E "^not ok|✖|expected|actual" | head -10; fail "hidden tests fail"; }
