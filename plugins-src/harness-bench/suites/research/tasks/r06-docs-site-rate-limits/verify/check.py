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


def num(v):
    if isinstance(v, str) and v.strip().isdigit():
        return int(v)
    if isinstance(v, float) and v.is_integer():
        return int(v)
    return v if isinstance(v, int) and not isinstance(v, bool) else None


EXP = {
    "get /v2/search": 1800,
    "post /v2/exports": 15,
    "get /v2/projects/{id}/members": 540,
    "post /v2/reports": 60,
    "patch /v2/webhooks/{id}": 1800,
}
a = load()
lim = a.get("limits")
if not isinstance(lim, dict):
    fail("limits must be an object")
got = {norm(k).strip("`"): num(v) for k, v in lim.items()}
for k, v in EXP.items():
    if got.get(k) != v:
        fail(f"{k}: {got.get(k)!r}")
if num(a.get("total")) != 4215:
    fail(f"total: {a.get('total')!r}")
need_sources(a, ["changelog.html", "plans.html", "addons.html", "rate-limits.html"])
print("ok")
