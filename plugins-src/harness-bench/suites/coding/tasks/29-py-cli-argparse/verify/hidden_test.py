import ast
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.getcwd()
NEW = os.path.join(HERE, "todo.py")
REF = os.path.join(HERE, "bench_reference_todo.py")

SESSIONS = [
    [["add", "buy", "milk"], ["add", "--priority", "1", "call", "mom"], ["add", "fix", "bike", "--priority", "5"],
     ["list"], ["done", "2"], ["list"], ["list", "--all"], ["list", "--priority", "5"], ["list", "--all", "--priority", "1"],
     ["list", "--priority", "3", "--all"], ["rm", "1"], ["list", "--all"], ["done", "07"], ["done", "3"], ["list"]],
    [["list"], ["done", "1"], ["rm", "4"], ["add", "x"], ["rm", "1"], ["list", "--all"], ["add", "y", "--priority", "2"], ["list"]],
]
BAD = [[], ["frob"], ["add"], ["add", "--priority", "9", "x"], ["add", "x", "--priority", "0"], ["add", "x", "--priority", "high"],
       ["add", "x", "--priority"], ["done"], ["done", "x"], ["done", "1", "2"], ["rm"], ["rm", "one"], ["list", "extra"],
       ["list", "--bogus"], ["--bogus", "list"], ["--file"]]


def run(script, args, cwd, env=None):
    e = dict(os.environ)
    e.pop("TODO_FILE", None)
    e.update(env or {})
    r = subprocess.run([sys.executable, script, *args], capture_output=True, text=True, cwd=cwd, env=e)
    return r.returncode, r.stdout, r.stderr


def snapshot(d):
    out = {}
    for name in sorted(os.listdir(d)):
        with open(os.path.join(d, name), "rb") as f:
            out[name] = f.read()
    return out


class SameBehavior(unittest.TestCase):
    def compare(self, mode):
        for session in SESSIONS:
            dirs = [tempfile.mkdtemp(), tempfile.mkdtemp()]
            try:
                for args in session:
                    results = []
                    for script, d in zip((REF, NEW), dirs):
                        if mode == "file":
                            full, env = ["--file", os.path.join(d, "data.json"), *args], None
                        elif mode == "env":
                            full, env = args, {"TODO_FILE": os.path.join(d, "env.json")}
                        else:
                            full, env = args, None
                        code, out, err = run(script, full, d, env)
                        results.append((code, out.replace(d, "<d>"), err.replace(d, "<d>")))
                    self.assertEqual(results[1], results[0], f"{mode}: todo.py {' '.join(args)}")
                    a, b = snapshot(dirs[0]), snapshot(dirs[1])
                    self.assertEqual(b, a, f"{mode}: files differ after todo.py {' '.join(args)}")
            finally:
                for d in dirs:
                    shutil.rmtree(d)

    def test_file_option(self):
        self.compare("file")

    def test_env_var(self):
        self.compare("env")

    def test_default_file(self):
        self.compare("default")


class NewBehavior(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        run(NEW, ["add", "keep", "me"], self.d)
        self.before = snapshot(self.d)

    def tearDown(self):
        shutil.rmtree(self.d)

    def test_version(self):
        self.assertEqual(run(NEW, ["--version"], self.d)[:2], (0, "todo 1.4.0\n"))

    def test_help(self):
        for args in (["--help"], ["-h"], ["add", "--help"], ["list", "--help"], ["done", "--help"], ["rm", "-h"]):
            code, out, _ = run(NEW, args, self.d)
            self.assertEqual(code, 0, args)
            self.assertIn("usage:", out, args)
        code, out, _ = run(NEW, ["--help"], self.d)
        for cmd in ("add", "list", "done", "rm"):
            self.assertIn(cmd, out)
        self.assertIn("--priority", run(NEW, ["add", "--help"], self.d)[1])
        self.assertIn("--all", run(NEW, ["list", "--help"], self.d)[1])

    def test_bad_usage(self):
        for args in BAD:
            code, out, err = run(NEW, args, self.d)
            self.assertEqual(code, 2, f"todo.py {' '.join(args)} exit code")
            self.assertEqual(out, "", f"todo.py {' '.join(args)} wrote to stdout")
            self.assertIn("usage", err.lower(), f"todo.py {' '.join(args)} stderr")
            self.assertEqual(snapshot(self.d), self.before, f"todo.py {' '.join(args)} changed files")


class Structure(unittest.TestCase):
    def test_uses_argparse(self):
        tree = ast.parse(open(NEW).read())
        names = {a.name for n in ast.walk(tree) if isinstance(n, (ast.Import, ast.ImportFrom)) for a in n.names}
        mods = {n.module for n in ast.walk(tree) if isinstance(n, ast.ImportFrom)}
        self.assertTrue("argparse" in names or "argparse" in mods, "argparse not imported")
        for n in ast.walk(tree):
            if isinstance(n, ast.Subscript) and isinstance(n.value, ast.Attribute) and n.value.attr == "argv":
                self.fail("sys.argv is still indexed by hand")
        self.assertIn("add_subparsers", open(NEW).read())


if __name__ == "__main__":
    unittest.main()
