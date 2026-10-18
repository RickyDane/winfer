import os
import subprocess
import sys
import tempfile
import unittest

TODO = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "todo.py")


def run(*args, env=None):
    return subprocess.run([sys.executable, TODO, *args], capture_output=True, text=True, env=env)


class Todo(unittest.TestCase):
    def test_add_list_done(self):
        with tempfile.TemporaryDirectory() as d:
            f = os.path.join(d, "t.json")
            self.assertEqual(run("--file", f, "add", "buy", "milk").stdout, "added #1: buy milk (p3)\n")
            self.assertEqual(run("--file", f, "add", "call", "mom", "--priority", "1").stdout, "added #2: call mom (p1)\n")
            self.assertEqual(run("--file", f, "done", "1").stdout, "done #1: buy milk\n")
            self.assertEqual(run("--file", f, "list").stdout, "[ ] #2   p1  call mom\n")
            self.assertEqual(run("--file", f, "list", "--all").stdout, "[ ] #2   p1  call mom\n[x] #1   p3  buy milk\n")


if __name__ == "__main__":
    unittest.main()
