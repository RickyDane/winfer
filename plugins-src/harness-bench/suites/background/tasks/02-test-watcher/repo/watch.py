"""Test watcher: reruns the tests whenever calc.py changes (like `vitest --watch`). Never exits."""
import importlib, os, sys, time

sys.path.insert(0, os.getcwd())
CASES = [
    ("average([1, 2, 3]) == 2", lambda c: c.average([1, 2, 3]) == 2),
    ("average([]) == 0", lambda c: c.average([]) == 0),
    ("clamp(5, 0, 10) == 5", lambda c: c.clamp(5, 0, 10) == 5),
    ("clamp(-1, 0, 10) == 0", lambda c: c.clamp(-1, 0, 10) == 0),
    ("clamp(11, 0, 10) == 10", lambda c: c.clamp(11, 0, 10) == 10),
    ("percent(1, 4) == 25", lambda c: c.percent(1, 4) == 25),
    ("percent(0, 0) == 0", lambda c: c.percent(0, 0) == 0),
]

def run():
    try:
        import calc
        importlib.reload(calc)
    except Exception as e:
        print(f"FAIL import: {e}", flush=True)
        return
    passed = 0
    for name, case in CASES:
        try:
            ok = case(calc)
        except Exception as e:
            ok = False
            name += f" ({type(e).__name__})"
        if ok:
            passed += 1
        else:
            print(f"  FAIL {name}", flush=True)
    print(f"Tests: {passed} passed, {len(CASES) - passed} failed", flush=True)
    with open(".git/watch.log", "a") as f:
        f.write(f"{passed}/{len(CASES)}\n")

open(".git/watch.pid", "w").write(str(os.getpid()))
print("watching calc.py", flush=True)
seen = None
while True:
    m = os.stat("calc.py").st_mtime
    if m != seen:
        seen = m
        print(f"[{time.strftime('%H:%M:%S')}] change detected, running tests…", flush=True)
        run()
    else:
        print(".", flush=True)
    time.sleep(0.3)
