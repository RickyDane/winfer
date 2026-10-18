"""Generates the project for this task into argv[1] (deterministic)."""

import os
import random
import sys

root = sys.argv[1]
rng = random.Random(14)


def write(path, text):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text)


write(".gitignore", "__pycache__/\n")
write("app/__init__.py", "")
write("app/http/__init__.py", "from app.http.client import Client\n")
write(
    "app/http/client.py",
    '''"""HTTP client facade over a transport callable(method, url, timeout, body)."""

import warnings


class Client:
    def __init__(self, transport):
        self.transport = transport

    def request(self, method, url, *, timeout=None, body=None):
        return self.transport(method, url, timeout, body)

    def fetch(self, url, timeout):
        """Deprecated: use request("GET", url, timeout=timeout)."""
        warnings.warn("Client.fetch is deprecated", DeprecationWarning, stacklevel=2)
        return self.request("GET", url, timeout=timeout)

    def fetch_all(self, urls, timeout):
        """GETs several URLs; returns the responses in order."""
        return [self.fetch(u, timeout) for u in urls]
''',
)
write(
    "app/cache.py",
    '''"""Tiny in-memory cache. Unrelated to the HTTP client."""


class Cache:
    def __init__(self):
        self._data = {}

    def store(self, key, value):
        self._data[key] = value

    def fetch(self, key, default=None):
        return self._data.get(key, default)
''',
)
write(
    "app/urls.py",
    '''BASE = "https://api.example.com"
TIMEOUT = 7


def build_url(*parts):
    return "/".join([BASE, *map(str, parts)])
''',
)

areas = ["users", "orders", "billing", "catalog", "search", "reports", "notify", "admin"]
for a in areas:
    write(f"app/{a}/__init__.py", "")

tests = []  # (module, function, args, expected url, expected timeout, kind)
for i in range(40):
    area = areas[i % len(areas)]
    mod = f"svc_{i:02d}"
    style = i % 6
    res = f"r{i}"
    lines = ['"""Generated service module."""', "", "from app.urls import TIMEOUT, build_url"]
    if i % 5 == 3:
        lines.append("from app.cache import Cache")
    lines += ["", ""]
    if style == 0:
        lines += [
            "def get(client, item_id):",
            f'    return client.fetch(build_url("{res}", item_id), 5)',
        ]
        tests.append((area, mod, "get", "client, 3", f"/{res}/3", 5))
    elif style == 1:
        lines += [
            "def get(client, item_id):",
            f'    url = build_url("{res}", item_id)',
            "    return client.fetch(url, timeout=TIMEOUT)",
        ]
        tests.append((area, mod, "get", "client, 3", f"/{res}/3", 7))
    elif style == 2:
        lines += [
            "class Service:",
            "    def __init__(self, client, timeout=4):",
            "        self.client = client",
            "        self.timeout = timeout",
            "",
            "    def get(self, item_id):",
            f'        return self.client.fetch(build_url("{res}", item_id), self.timeout)',
        ]
        tests.append((area, mod, "Service(client).get", "3", f"/{res}/3", 4))
    elif style == 3:
        lines += [
            "class Service:",
            "    def __init__(self, client):",
            "        self._http = client",
            "",
            "    def get(self, item_id):",
            "        resp = self._http.fetch(",
            f'            build_url("{res}", item_id, "detail"),',
            "            TIMEOUT * 2,",
            "        )",
            "        return resp",
        ]
        tests.append((area, mod, "Service(client).get", "3", f"/{res}/3/detail", 14))
    elif style == 4:
        lines += [
            "def get(api, item_id, timeout=9):",
            f'    return api.fetch(f"{{build_url(\'{res}\')}}?id={{item_id}}", timeout)',
        ]
        tests.append((area, mod, "get", "client, 3", f"/{res}?id=3", 9))
    else:
        lines += [
            "def get_many(client, ids):",
            f'    return client.fetch_all([build_url("{res}", i) for i in ids], 2)',
        ]
        tests.append((area, mod, "get_many", "client, [3]", f"/{res}/3", 2))
    if i % 5 == 3:
        lines += [
            "",
            "",
            "_cache = Cache()",
            "",
            "",
            "def cached(client, item_id):",
            "    hit = _cache.fetch(item_id)",
            "    if hit is None:",
            f'        hit = client.fetch(build_url("{res}", "c", item_id), 1)',
            "        _cache.store(item_id, hit)",
            "    return _cache.fetch(item_id)",
        ]
        tests.append((area, mod, "cached", "client, 3", f"/{res}/c/3", 1))
    lines.append("")
    write(f"app/{area}/{mod}.py", "\n".join(lines))

t = [
    "import unittest",
    "import warnings",
    "",
    "from app.cache import Cache",
    "from app.http import Client",
    "from app.urls import BASE",
]
for area, mod, *_ in sorted({(x[0], x[1]) for x in tests}):
    t.append(f"from app.{area} import {mod}")
t += [
    "",
    "",
    "class Recorder:",
    "    def __init__(self):",
    "        self.calls = []",
    "",
    "    def __call__(self, method, url, timeout, body):",
    "        self.calls.append((method, url, timeout, body))",
    '        return {"url": url}',
    "",
    "",
    "class CallSitesTest(unittest.TestCase):",
    "    def setUp(self):",
    '        warnings.simplefilter("ignore", DeprecationWarning)',
    "",
]
for n, (area, mod, fn, args, url, timeout) in enumerate(tests):
    t += [
        f"    def test_{n:02d}_{mod}_{fn.split('.')[-1]}(self):",
        "        rec = Recorder()",
        "        client = Client(rec)",
        f"        out = {mod}.{fn}({args})",
        f'        self.assertEqual(rec.calls, [("GET", BASE + "{url}", {timeout}, None)])',
        "        self.assertIsNotNone(out)",
        "",
    ]
t += [
    "    def test_cache(self):",
    "        c = Cache()",
    '        c.store("k", 1)',
    '        self.assertEqual(c.fetch("k"), 1)',
    '        self.assertEqual(c.fetch("x", 2), 2)',
    "",
    "",
    'if __name__ == "__main__":',
    "    unittest.main()",
    "",
]
write("tests/test_calls.py", "\n".join(t))
write(
    "docs/http.md",
    "# HTTP\n\nUse `client.request(\"GET\", url, timeout=...)`. `client.fetch_all(urls, timeout)` GETs several URLs.\n",
)
