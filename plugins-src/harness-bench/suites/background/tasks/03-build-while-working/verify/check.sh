#!/bin/sh
fail() { echo "$1"; exit 1; }
grep -q 'VERSION = "1.5.0"' version.py || fail "version not bumped"
head -5 CHANGELOG.md | grep -q "^## 1.5.0" || fail "changelog section missing on top"
grep -q "^- Faster startup.$" CHANGELOG.md || fail "changelog entry missing"
[ -f .build-id ] || fail "build never finished"
[ "$(tr -d ' \n' < answer.txt 2>/dev/null)" = "$(cat .build-id)" ] || fail "answer.txt doesn't hold the build id"
true
