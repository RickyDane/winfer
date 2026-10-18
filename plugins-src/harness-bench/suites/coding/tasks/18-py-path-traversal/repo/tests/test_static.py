import os
import tempfile
import unittest

from fileserver.static import handle


class StaticTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = os.path.join(self.tmp.name, "public")
        os.makedirs(os.path.join(self.root, "docs"))
        self.write("index.html", "<h1>home</h1>")
        self.write("docs/index.html", "docs home")
        self.write("docs/my file.txt", "spaced")

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, rel, text):
        with open(os.path.join(self.root, rel), "w") as f:
            f.write(text)

    def test_index(self):
        status, headers, body = handle(self.root, "GET", "/")
        self.assertEqual((status, body), (200, b"<h1>home</h1>"))
        self.assertEqual(headers["Content-Type"], "text/html")

    def test_nested_and_encoded(self):
        self.assertEqual(handle(self.root, "GET", "/docs/")[2], b"docs home")
        self.assertEqual(handle(self.root, "GET", "/docs/my%20file.txt?x=1")[2], b"spaced")

    def test_dotdot_inside_root(self):
        self.assertEqual(handle(self.root, "GET", "/docs/../index.html")[2], b"<h1>home</h1>")

    def test_missing_and_method(self):
        self.assertEqual(handle(self.root, "GET", "/nope.txt")[0], 404)
        self.assertEqual(handle(self.root, "POST", "/")[0], 405)

    def test_escape_forbidden(self):
        self.assertEqual(handle(self.root, "GET", "/../secret.txt")[0], 403)


if __name__ == "__main__":
    unittest.main()
