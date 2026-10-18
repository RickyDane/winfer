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


def ident(s):
    s = norm(s).strip("`").replace("()", "")
    for p in ("lumen.", "client.", "lumen."):
        if s.startswith(p):
            s = s[len(p):]
    return s.split("=")[0].strip()


def num(v):
    if isinstance(v, str) and v.strip().isdigit():
        return int(v)
    return v if isinstance(v, int) and not isinstance(v, bool) else None


a = load()
if norm(a.get("latest_stable_release", "")).lstrip("v") != "3.4.0":
    fail(f"latest_stable_release: {a.get('latest_stable_release')!r}")
if num(a.get("default_timeout_seconds")) != 15:
    fail(f"default_timeout_seconds: {a.get('default_timeout_seconds')!r}")
if ident(a.get("streaming_method", "")) != "open_stream":
    fail(f"streaming_method: {a.get('streaming_method')!r}")
if ident(a.get("status_exception", "")) != "statuserror":
    fail(f"status_exception: {a.get('status_exception')!r}")
if ident(a.get("disable_tls_verification_param", "")) != "tls_verify":
    fail(f"disable_tls_verification_param: {a.get('disable_tls_verification_param')!r}")
if num(a.get("default_max_attempts")) != 3:
    fail(f"default_max_attempts: {a.get('default_max_attempts')!r}")
if norm(a.get("min_python", "")).lstrip("python ").strip("+") != "3.10":
    fail(f"min_python: {a.get('min_python')!r}")
need_sources(a, ["changelog.html"])
if not any(path_of(s).startswith("v3/") for s in a.get("sources", []) if isinstance(s, str)):
    fail("sources must include the 3.x pages used")
print("ok")
