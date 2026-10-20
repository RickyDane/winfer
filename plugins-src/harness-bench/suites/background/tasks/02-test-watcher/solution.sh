python3 watch.py > /dev/null 2>&1 &
pid=$!
sleep 1
cat > calc.py <<'PY'
def average(xs):
    return sum(xs) / len(xs) if xs else 0


def clamp(x, lo, hi):
    return max(lo, min(x, hi))


def percent(part, whole):
    return round(100 * part / whole) if whole else 0
PY
sleep 1.5
kill $pid; wait $pid 2>/dev/null
exit 0
