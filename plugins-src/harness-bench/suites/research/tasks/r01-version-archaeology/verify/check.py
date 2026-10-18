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


a = load()
exp = {
    "original_name": "cache.max_age",
    "introduced_in": "1.4.0",
    "renamed_in": "2.3.0",
    "alias_removed_in": "3.0.0",
    "current_name": "cache.ttl",
    "latest_release": "4.2.1",
}
for k, v in exp.items():
    got = norm(a.get(k, "")).strip("`").lstrip("v")
    if got != v:
        fail(f"{k}: {a.get(k)!r}")
d = a.get("current_default_seconds")
if isinstance(d, str) and d.strip().isdigit():
    d = int(d)
if d != 43200 or isinstance(d, bool):
    fail(f"current_default_seconds: {a.get('current_default_seconds')!r}")
need_sources(a, ["CHANGELOG.md"])
print("ok")
