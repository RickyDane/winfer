#!/bin/sh
fail() { echo "$1"; exit 1; }
ignore='__pycache__|\.pyc$|\.pytest_cache|\.bench-output'
[ -f QUESTIONS.md ] || fail "no QUESTIONS.md (the spec contradicts itself: 3.2 vs 5.1)"
other=$(git status --porcelain --untracked-files=all | grep -vE "$ignore" | grep -vx '?? QUESTIONS.md')
[ -z "$other" ] || { echo "$other" | head -5; fail "changed files other than adding QUESTIONS.md"; }
grep -q '3\.2' QUESTIONS.md || fail "QUESTIONS.md does not cite section 3.2"
grep -q '5\.1' QUESTIONS.md || fail "QUESTIONS.md does not cite section 5.1"
grep -qi 'tax\|vat' QUESTIONS.md || fail "QUESTIONS.md does not mention tax/VAT"
grep -qi 'discount\|coupon' QUESTIONS.md || fail "QUESTIONS.md does not mention the discount/coupon"
