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
    if isinstance(v, str) and v.strip().lstrip("#").isdigit():
        return int(v.strip().lstrip("#"))
    return v if isinstance(v, int) and not isinstance(v, bool) else None


def ver(v):
    return norm(v).lstrip("v")


a = load()
for k, v in {"introduced_by_pr": 380, "final_fix_pr": 431, "backport_pr": 437}.items():
    if num(a.get(k)) != v:
        fail(f"{k}: {a.get(k)!r}")
for k, v in {"first_affected_release": "2.6.0", "first_fixed_release": "2.8.0", "backport_release": "2.7.5"}.items():
    if ver(a.get(k, "")) != v:
        fail(f"{k}: {a.get(k)!r}")
w = a.get("workaround")
if not isinstance(w, dict) or norm(w.get("setting", "")).strip("`") != "upload.part_size_mb" or num(w.get("value")) != 256:
    fail(f"workaround: {w!r}")
need_sources(a, ["export/issues/412.json", "export/releases.json"])
print("ok")
