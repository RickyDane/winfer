import os
import tempfile
import unittest
import urllib.parse

from fileserver.static import Forbidden, handle, resolve

SECRET = b"TOP-SECRET-CONTENT"


class Hidden(unittest.TestCase):
    def setUp(self):
        # On macOS the temp dir itself is behind a symlink (/var -> /private/var).
        self.tmp = tempfile.TemporaryDirectory()
        base = self.tmp.name
        if os.name == "nt":
            # %TEMP% can be an 8.3 short path, use forward slashes or the wrong case; symlink
            # targets built from it then don't match the root's real path. Use the canonical form.
            base = os.path.realpath(base)
        self.root = os.path.join(base, "public")
        self.outside = os.path.join(base, "private")
        sibling = os.path.join(base, "public2")
        for d in (self.root, os.path.join(self.root, "docs", "deep"), os.path.join(self.root, "a", "b"), self.outside, sibling):
            os.makedirs(d, exist_ok=True)
        self.put(self.root, "index.html", b"home")
        self.put(self.root, "docs/index.html", b"docs home")
        self.put(self.root, "docs/deep/x.txt", b"deep x")
        self.put(self.root, "docs/my file.txt", b"spaced")
        self.put(self.root, "docs/..hidden.txt", b"dotdot name")
        self.put(self.root, "docs/a..b.txt", b"dots inside")
        self.put(self.root, "100%.txt", b"percent")
        self.put(self.outside, "secret.txt", SECRET)
        self.put(self.outside, "index.html", SECRET)
        self.put(sibling, "secret.txt", SECRET)
        self.put(base, "secret.txt", SECRET)
        # Windows needs to know a link points at a directory, or it can't be traversed.
        os.symlink(self.outside, os.path.join(self.root, "evil_dir"), target_is_directory=True)
        os.symlink(os.path.join(self.outside, "secret.txt"), os.path.join(self.root, "evil.txt"))
        os.symlink(os.path.join(self.root, "docs", "deep", "x.txt"), os.path.join(self.root, "good.txt"))
        os.symlink(os.path.join(self.root, "docs"), os.path.join(self.root, "a", "docs_link"), target_is_directory=True)
        os.symlink(os.path.join("..", "..", "private", "secret.txt"), os.path.join(self.root, "docs", "rel_evil.txt"))

    def tearDown(self):
        self.tmp.cleanup()

    def put(self, d, rel, data):
        p = os.path.join(d, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data)

    def ok(self, url, body, root=None):
        root = root or self.root
        status, headers, got = handle(root, "GET", url)
        if (status, got) != (200, body):
            try:
                r = repr(resolve(root, url))
            except Exception as e:  # noqa: BLE001
                r = f"{type(e).__name__}: {e}"
            url = f"{url}\n  resolve() -> {r}\n  realpath(root) = {os.path.realpath(root)}\n  realpath(file) = {os.path.realpath(os.path.join(root, url.lstrip('/').split('?')[0]))}"
        self.assertEqual((status, got), (200, body), url)

    def blocked(self, url):
        try:
            status, headers, body = handle(self.root, "GET", url)
        except Exception as e:  # noqa: BLE001
            self.fail(f"{url!r} raised {type(e).__name__}: {e}")
        self.assertIn(status, (400, 403, 404), url)
        self.assertNotIn(SECRET, body, url)
        try:
            r = resolve(self.root, url)
        except Forbidden:
            return
        except ValueError:
            return
        if r is not None:
            real = os.path.realpath(r)
            self.assertEqual(os.path.commonpath([os.path.realpath(self.root), real]), os.path.realpath(self.root), f"resolve({url!r}) escaped: {r}")

    def test_legit(self):
        self.ok("/", b"home")
        self.ok("/index.html", b"home")
        self.ok("/docs", b"docs home")
        self.ok("/docs/", b"docs home")
        self.ok("/docs/deep/x.txt", b"deep x")
        self.ok("/docs/deep/x.txt?download=1#top", b"deep x")
        self.ok("/docs/my%20file.txt", b"spaced")
        self.ok("/docs/..hidden.txt", b"dotdot name")
        self.ok("/docs/a..b.txt", b"dots inside")
        self.ok("/100%25.txt", b"percent")
        self.ok("/docs/deep/../../index.html", b"home")
        self.ok("/./docs/./deep/x.txt", b"deep x")
        self.ok("/good.txt", b"deep x")
        self.ok("/a/docs_link/deep/x.txt", b"deep x")
        self.ok("/a/b/../docs_link/index.html", b"docs home")
        self.ok("/docs/deep/x.txt", b"deep x", root=self.root + "/")
        self.ok("/docs/deep/x.txt", b"deep x", root=os.path.relpath(self.root))

    def test_head_and_errors_unchanged(self):
        status, headers, body = handle(self.root, "HEAD", "/docs/deep/x.txt")
        self.assertEqual((status, body, headers["Content-Length"]), (200, b"", "6"))
        self.assertEqual(handle(self.root, "GET", "/missing.txt")[0], 404)
        self.assertEqual(handle(self.root, "GET", "/docs/deep/nope")[0], 404)
        self.assertEqual(handle(self.root, "DELETE", "/")[0], 405)

    def test_plain_dotdot(self):
        for u in ["/../secret.txt", "/../private/secret.txt", "/docs/../../secret.txt", "/../../../../../../etc/passwd",
                  "/docs/deep/../../../private/secret.txt", "/..", "/../public2/secret.txt"]:
            self.blocked(u)

    def test_encoded(self):
        for u in ["/%2e%2e/secret.txt", "/%2E%2E/private/secret.txt", "/docs/%2e%2e/%2e%2e/secret.txt",
                  "/.%2e/secret.txt", "/%2e./secret.txt", "/..%2fsecret.txt", "/docs/..%2F..%2Fsecret.txt",
                  "/%2e%2e%2f%2e%2e%2fprivate%2fsecret.txt", "/docs%2f..%2f..%2fsecret.txt",
                  "/%252e%252e/secret.txt", "/../public2/secret.txt", "/%2e%2e/public2/secret.txt"]:
            self.blocked(u)

    def test_absolute(self):
        secret = os.path.join(self.outside, "secret.txt")
        for u in ["/" + secret, "//" + secret.lstrip("/"), "/" + urllib.parse.quote(secret, safe=""),
                  "/%2F" + urllib.parse.quote(secret.lstrip("/")), "///etc/passwd", "/%2fetc%2fpasswd"]:
            self.blocked(u)

    def test_symlinks_out(self):
        for u in ["/evil.txt", "/evil_dir/secret.txt", "/evil_dir/", "/evil_dir", "/docs/rel_evil.txt"]:
            self.blocked(u)

    def test_weird(self):
        for u in ["/docs/%00/../../secret.txt", "/secret.txt%00.html", "/..\\secret.txt", "/%5c..%5csecret.txt"]:
            self.blocked(u)


if __name__ == "__main__":
    unittest.main()
