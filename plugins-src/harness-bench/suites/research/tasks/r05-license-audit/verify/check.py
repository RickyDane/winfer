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


EXP = {
    ("qs-tiny", "6.0.2"): (r"(?<![a-z])mpl-2\.0", ["express-lite"]),
    ("color-space", "2.0.0"): (r"(?<![a-z])lgpl-3\.0|(?<![a-z])lgpl-?3|lesser general public license", ["chartkit"]),
    ("pdf-mini", "0.4.0"): (r"(?<![a-z])gpl-2\.0", ["chartkit"]),
    ("xml-sax-lite", "0.7.3"): (r"(?<![a-z])agpl-3\.0|(?<![a-z])agpl-?3|affero", ["auth-guard", "md-render"]),
    ("tiny-glob", "0.9.1"): (r"(?<![a-z])gpl-2\.0|(?<![a-z])gpl-?2", ["csv-stream"]),
}
a = load()
items = a.get("copyleft")
if not isinstance(items, list):
    fail("copyleft must be a list")
seen = {}
for it in items:
    if not isinstance(it, dict):
        fail("each copyleft entry must be an object")
    key = (norm(it.get("name", "")), norm(it.get("version", "")).lstrip("v"))
    seen[key] = it
missing = sorted(set(EXP) - set(seen))
extra = sorted(set(seen) - set(EXP))
if missing or extra:
    fail(f"missing: {missing}  extra: {extra}")
for key, (lic, via) in EXP.items():
    it = seen[key]
    if not re.search(lic, norm(it.get("license", ""))):
        fail(f"{key[0]} license: {it.get('license')!r}")
    v = it.get("via")
    if not isinstance(v, list) or sorted(norm(x) for x in v) != via:
        fail(f"{key[0]} via: {v!r}")
print("ok")
