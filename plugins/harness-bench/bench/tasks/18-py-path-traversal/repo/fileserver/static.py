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
    if "/../" in path or path.endswith("/.."):
        # Only allow `..` that stays inside the root.
        parts = []
        for seg in path.split("/"):
            if seg == "..":
                if not parts:
                    raise Forbidden(url_path)
                parts.pop()
            elif seg and seg != ".":
                parts.append(seg)
        path = "/" + "/".join(parts)
    path = urllib.parse.unquote(path)
    full = os.path.join(root, path[1:])
    if os.path.isdir(full):
        full = os.path.join(full, "index.html")
    if not os.path.isfile(full):
        return None
    return full


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
