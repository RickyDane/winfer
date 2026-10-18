"""Serves files from a root directory.

URL paths map onto the root: `/docs/a.txt` -> `<root>/docs/a.txt`. The query string and
fragment are ignored and the path is percent-decoded (`/my%20file.txt`). A directory serves
its `index.html`. `..` segments are resolved like a browser would, so `/docs/../index.html`
is `<root>/index.html`. Symlinks inside the root are followed.

`resolve()` returns the file to serve, `None` if there's no such file, or raises `Forbidden`
if the request isn't allowed. `handle()` turns that into an HTTP response.
"""

import mimetypes
import os
import urllib.parse


class Forbidden(Exception):
    pass


def resolve(root, url_path):
    path = url_path.split("?", 1)[0].split("#", 1)[0]
    # Decode first, then check: `%2e%2e` and `%2f` must not slip past the checks.
    path = urllib.parse.unquote(path)
    if "\x00" in path or "\\" in path:
        raise Forbidden(url_path)
    parts = []
    for seg in path.split("/"):
        if seg == "..":
            if not parts:
                raise Forbidden(url_path)
            parts.pop()
        elif seg and seg != ".":
            parts.append(seg)
    real_root = os.path.realpath(root)
    full = os.path.join(real_root, *parts)
    # Follow symlinks and make sure the result is still inside the root.
    real = os.path.realpath(full)
    if os.path.commonpath([real_root, real]) != real_root:
        raise Forbidden(url_path)
    if os.path.isdir(real):
        real = os.path.realpath(os.path.join(real, "index.html"))
        if os.path.commonpath([real_root, real]) != real_root:
            raise Forbidden(url_path)
    if not os.path.isfile(real):
        return None
    return real


def handle(root, method, url_path):
    """Returns (status, headers, body)."""
    if method not in ("GET", "HEAD"):
        return 405, {"Allow": "GET, HEAD"}, b""
    try:
        full = resolve(root, url_path)
    except Forbidden:
        return 403, {}, b"forbidden"
    if full is None:
        return 404, {}, b"not found"
    with open(full, "rb") as f:
        body = f.read()
    ctype = mimetypes.guess_type(full)[0] or "application/octet-stream"
    headers = {"Content-Type": ctype, "Content-Length": str(len(body))}
    return 200, headers, b"" if method == "HEAD" else body
