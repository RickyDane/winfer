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
got = a.get("violated_sections")
if not isinstance(got, list):
    fail("violated_sections must be a list")
norm_got = set()
for s in got:
    s = norm(s).replace("section", "").replace("§", "").strip()
    norm_got.add(s)
exp = {"3.2", "3.4", "4.1", "5.3", "6.2", "7.2"}
if norm_got != exp:
    print(f"missing: {sorted(exp - norm_got)}  extra: {sorted(norm_got - exp)}")
    sys.exit(1)
print("ok")
