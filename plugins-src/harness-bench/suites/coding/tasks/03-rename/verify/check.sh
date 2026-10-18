#!/bin/sh
fail() { echo "$1"; exit 1; }
# No compute_tax left (compute_tax_free must stay).
if grep -rnwE "compute_tax" --include=*.py --include=*.md . | grep -v compute_tax_free | grep -q .; then
  grep -rnwE "compute_tax" --include=*.py --include=*.md . | grep -v compute_tax_free | head -5
  fail "compute_tax still referenced"
fi
grep -q "def compute_tax_free" app/billing/tax.py || fail "compute_tax_free was renamed"
grep -q "def calculate_tax" app/billing/tax.py || fail "calculate_tax not defined"
n=$(grep -rlw calculate_tax app | wc -l | tr -d ' ')
[ "$n" -ge 10 ] || fail "only $n files use calculate_tax"
grep -q "calculate_tax(cents, country)" docs/billing.md || fail "docs not updated"
# The tests import modules (not compute_tax), so they must still pass; the tests may be updated.
python3 -m unittest discover -s tests -q 2>&1 || fail "tests fail"
python3 -c "from app.billing.tax import calculate_tax as c; assert c(1000,'FR')==200 and c(1,'DE')==0" || fail "behavior changed"
