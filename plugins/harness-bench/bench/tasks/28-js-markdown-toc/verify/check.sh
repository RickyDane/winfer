#!/bin/sh
fail() { echo "$1"; exit 1; }
[ -f toc.mjs ] || fail "no toc.mjs"
t=$(mktemp -d)
for f in "$VERIFY_DIR"/fixtures/*.in.md; do
  n=$(basename "$f" .in.md)
  cp "$f" "$t/$n.md"
  node toc.mjs "$t/$n.md" 2>"$t/err" || fail "fixture $n: exit code $? ($(head -c 300 "$t/err"))"
  cmp -s "$t/$n.md" "$VERIFY_DIR/fixtures/$n.out.md" || { diff "$VERIFY_DIR/fixtures/$n.out.md" "$t/$n.md" | head -20; fail "fixture $n: wrong output"; }
  node toc.mjs "$t/$n.md" 2>/dev/null || fail "fixture $n: second run failed"
  cmp -s "$t/$n.md" "$VERIFY_DIR/fixtures/$n.out.md" || fail "fixture $n: not idempotent"
done
cp "$VERIFY_DIR/fixtures/nomarkers.md" "$t/nm.md"
node toc.mjs "$t/nm.md" 2>"$t/err" >/dev/null; code=$?
[ $code -eq 1 ] || fail "no markers: exit code $code, want 1"
[ -s "$t/err" ] || fail "no markers: nothing on stderr"
cmp -s "$t/nm.md" "$VERIFY_DIR/fixtures/nomarkers.md" || fail "no markers: file was changed"
# The example in the repo must round-trip too.
cp docs/example.md "$t/ex.md"; git show HEAD:docs/example.md > "$t/ex.orig.md"
node toc.mjs "$t/ex.md" && cmp -s "$t/ex.md" "$t/ex.orig.md" || fail "docs/example.md not reproduced"
rm -rf "$t"
