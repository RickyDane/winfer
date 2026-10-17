import json
import os
import subprocess
import sys
import tempfile
import unittest


def run(*args, env):
    r = subprocess.run([sys.executable, "todo.py", *args], capture_output=True, text=True, env=env)
    return r.returncode, r.stdout.strip().splitlines()


class Hidden(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.env = {**os.environ, "TODO_FILE": os.path.join(self.dir, "t.json")}

    def test_priorities_and_sort(self):
        run("add", "a", "--priority", "low", env=self.env)
        run("add", "b", env=self.env)
        run("add", "c", "--priority", "high", env=self.env)
        run("add", "d", "--priority", "high", env=self.env)
        _, out = run("list", env=self.env)
        self.assertEqual(out, ["1. [low] [ ] a", "2. [med] [ ] b", "3. [high] [ ] c", "4. [high] [ ] d"])
        _, out = run("list", "--sort", "priority", env=self.env)
        self.assertEqual([l.split(".")[0] for l in out], ["3", "4", "2", "1"])

    def test_bad_priority_fails(self):
        code, _ = run("add", "x", "--priority", "urgent", env=self.env)
        self.assertNotEqual(code, 0)

    def test_clear_done_and_legacy(self):
        with open(self.env["TODO_FILE"], "w") as f:
            json.dump([{"id": 1, "text": "old", "done": True}, {"id": 2, "text": "keep", "done": False}], f)
        _, out = run("list", env=self.env)
        self.assertEqual(out, ["1. [med] [x] old", "2. [med] [ ] keep"])
        _, out = run("clear-done", env=self.env)
        self.assertEqual(out, ["Removed 1"])
        _, out = run("list", env=self.env)
        self.assertEqual(out, ["2. [med] [ ] keep"])
        _, out = run("add", "new", env=self.env)
        self.assertEqual(out, ["Added 3"])


if __name__ == "__main__":
    unittest.main()
