import json, re, sys


def fail(msg):
    print(msg)
    sys.exit(1)


def norm(s):
    return re.sub(r"\s+", " ", str(s)).strip().lower()


def load():
    try:
        with open("ANSWER.json") as f:
            a = json.load(f)
    except Exception as e:
        fail(f"ANSWER.json is not valid JSON: {e}")
    if not isinstance(a, dict):
        fail("ANSWER.json must be an object")
    return a


def path_of(s):
    s = norm(s).replace("\\", "/")
    s = re.sub(r"^[a-z]+://[^/]+", "", s)
    s = s.split("#")[0].split("?")[0]
    while s.startswith("./"):
        s = s[2:]
    return s.lstrip("/")


def need_sources(a, required, key="sources"):
    got = a.get(key)
    if not isinstance(got, list):
        fail(f"'{key}' must be a list")
    paths = [path_of(x) for x in got if isinstance(x, str)]
    for r in required:
        r = path_of(r)
        if not any(p == r or p.endswith("/" + r) for p in paths):
            fail(f"sources missing {r}")


import datetime as dt

FULL = "4c9a1f7e3b2d8a6c5f1e0d9b8a7c6e5f4d3c2b1a"
a = load()
c = norm(a.get("root_cause_commit", ""))
if len(c) < 7 or not FULL.startswith(c):
    fail(f"root_cause_commit: {a.get('root_cause_commit')!r}")


def utc(v, want):
    s = str(v).strip().replace(" ", "T")
    if s.endswith("Z") or s.endswith("z"):
        s = s[:-1] + "+00:00"
    try:
        t = dt.datetime.fromisoformat(s)
    except ValueError:
        return False
    if t.tzinfo is None or t.utcoffset() != dt.timedelta(0):
        return False
    return t.replace(tzinfo=None) == dt.datetime.fromisoformat(want)


if not utc(a.get("start_utc", ""), "2026-03-14T21:42:49"):
    fail(f"start_utc: {a.get('start_utc')!r}")
if not utc(a.get("end_utc", ""), "2026-03-14T22:32:14"):
    fail(f"end_utc: {a.get('end_utc')!r}")
s = a.get("affected_services")
exp = ["api-gateway", "billing", "checkout", "db-proxy", "notification-worker"]
if not isinstance(s, list) or sorted({norm(x) for x in s}) != exp or len(s) != len(exp):
    fail(f"affected_services: {s!r}")
print("ok")
