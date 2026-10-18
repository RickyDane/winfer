"""Regression checks for cost fallback; no model calls or credentials required."""

import json
import os
import subprocess
import sys
import time
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import run as bench


class CostTests(unittest.TestCase):
    def test_cached_and_prefixed_model(self):
        self.assertAlmostEqual(bench.estimate_cost("openai/gpt-6.1-sol", 1_000_000, 500_000, 100_000), 2.05)

    def test_unknown_model_or_missing_usage(self):
        self.assertIsNone(bench.estimate_cost("unknown", 100, 0, 10))
        self.assertIsNone(bench.estimate_cost("sonnet", 100, 0, 10))
        self.assertIsNone(bench.estimate_cost("gpt-6.1-sol", 0, 0, 0))

    def run_with_output(self, parser, output, model="gpt-6.1-sol"):
        with tempfile.TemporaryDirectory() as folder:
            task = Path(folder)
            (task / "task.md").write_text("Synthetic pricing check")
            spec = {"parser": parser, "argv": ["synthetic"], "model": "unknown"}
            responses = [subprocess.CompletedProcess([], 0, output, ""), subprocess.CompletedProcess([], 0, "", "")]
            with patch.object(bench, "prepare"), patch.object(bench.subprocess, "run", side_effect=responses):
                return bench.run_one("synthetic", spec, task, 10, False, model)

    def test_codex_fallback_uses_selected_model(self):
        output = json.dumps({"type": "turn.completed", "usage": {
            "input_tokens": 1_000_000, "cached_input_tokens": 500_000, "output_tokens": 100_000,
        }})
        result = self.run_with_output("codex", output, "openai/gpt-6.1-sol")
        self.assertAlmostEqual(result["cost"], 2.05)
        self.assertTrue(result["cost_estimated"])
        self.assertTrue(result["pass"])

    def test_reported_cost_including_zero_is_preserved(self):
        for cost in (0, 0.123):
            with self.subTest(cost=cost):
                output = json.dumps({"type": "usage", "input": 1_000_000, "cached": 500_000, "output": 100_000, "cost": cost})
                result = self.run_with_output("winfer", output)
                self.assertEqual(result["cost"], cost)
                self.assertFalse(result["cost_estimated"])

    def test_unpriced_runs_do_not_claim_an_estimate(self):
        for output, model in (("", "gpt-6.1-sol"), (json.dumps({"type": "usage", "input": 100}), "unknown")):
            with self.subTest(model=model):
                result = self.run_with_output("winfer", output, model)
                self.assertIsNone(result["cost"])
                self.assertFalse(result["cost_estimated"])


class EffortTests(unittest.TestCase):
    def test_argv_goes_before_the_prompt(self):
        spec = bench.CATALOG["claude"]
        argv = bench.harness_argv(spec, "sonnet", "high", "do it")
        self.assertEqual(argv[-3:], ["--effort", "high", "do it"])
        self.assertIn("sonnet", argv)

    def test_model_template(self):
        argv = bench.harness_argv(bench.CATALOG["opencode"], "openai/gpt-6.1-sol", "xhigh", "do it")
        self.assertIn("openai/gpt-6.1-sol#xhigh", argv)

    def test_no_effort_leaves_argv_alone(self):
        for name in ("claude", "codex", "opencode"):
            spec = bench.CATALOG[name]
            expected = [a.replace("{prompt}", "p").replace("{model}", "m") for a in spec["argv"]]
            self.assertEqual(bench.harness_argv(spec, "m", "", "p"), expected)

    def test_every_effort_spec_has_levels_and_a_placeholder(self):
        for name, spec in bench.CATALOG.items():
            how = spec.get("effort") if isinstance(spec, dict) else None
            if how:
                with self.subTest(name=name):
                    self.assertTrue(how["levels"])
                    self.assertIn("{effort}", json.dumps(how.get("argv") or how.get("model")))

    def test_result_records_effort(self):
        with tempfile.TemporaryDirectory() as folder:
            task = Path(folder)
            (task / "task.md").write_text("Synthetic effort check")
            spec = {"parser": "none", "argv": ["synthetic", "{prompt}"], "effort": {"argv": ["--effort", "{effort}"], "levels": ["high"]}}
            responses = [subprocess.CompletedProcess([], 0, "", ""), subprocess.CompletedProcess([], 0, "", "")]
            with patch.object(bench, "prepare"), patch.object(bench.subprocess, "run", side_effect=responses) as run:
                result = bench.run_one("synthetic", spec, task, 10, False, "m", "high")
            self.assertEqual(run.call_args_list[0].args[0][-3:-1], ["--effort", "high"])
            self.assertEqual(result["effort"], "high")


class InstalledTests(unittest.TestCase):
    def test_missing_cli_or_required_cli_counts_as_not_installed(self):
        have = {"winfer", "codex"}
        specs = {
            "winfer-claude": {"argv": ["{winfer}"], "requires": ["claude"]},
            "winfer-codex": {"argv": ["{winfer}"], "requires": ["codex"]},
            "claude": {"argv": ["claude"]},
            "codex": {"argv": ["codex"]},
            "oracle": {"argv": []},
        }
        with patch.object(bench.shutil, "which", side_effect=lambda c: f"/bin/{c}" if c in have else None), \
             patch.object(bench, "winfer_bin", return_value="/bin/winfer"), patch.object(bench, "all_tasks", return_value=[]), \
             patch.object(bench, "suite_names", return_value=[]):
            got = {h["id"]: h["installed"] for h in bench.listing(specs)["harnesses"]}
        self.assertEqual(got, {"winfer-claude": False, "winfer-codex": True, "claude": False, "codex": True, "oracle": True})


class SuiteTests(unittest.TestCase):
    def make(self, root, suite, *tasks, info=None):
        for t in tasks:
            (root / suite / "tasks" / t).mkdir(parents=True)
            (root / suite / "tasks" / t / "task.md").write_text(t)
        if info:
            (root / suite / "suite.json").write_text(json.dumps(info))

    def test_known_suites_first_then_alphabetical(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for s in ("zeta", "discipline", "coding", "alpha"):
                self.make(root, s, "01-x")
            (root / "not-a-suite").mkdir()
            self.assertEqual(bench.suite_names(root), ["coding", "discipline", "alpha", "zeta"])

    def test_all_tasks_filters_and_suite_of(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.make(root, "coding", "02-b", "01-a")
            self.make(root, "research", "r01-c")
            (root / "research" / "tasks" / "r02-half-written").mkdir()
            self.assertEqual([t.name for t in bench.all_tasks(root)], ["01-a", "02-b", "r01-c"])
            only = bench.all_tasks(root, ["research"])
            self.assertEqual([t.name for t in only], ["r01-c"])
            self.assertEqual(bench.suite_of(only[0]), "research")

    def test_suite_info_defaults(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.make(root, "research", "r01", info={"label": "Research", "description": "d"})
            self.make(root, "misc", "m01")
            self.assertEqual(bench.suite_info("research", root), {"id": "research", "label": "Research", "description": "d"})
            self.assertEqual(bench.suite_info("misc", root)["label"], "Misc")

    def test_repo_suites_have_unique_task_ids(self):
        ids = [t.name for t in bench.all_tasks()]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertIn("coding", bench.suite_names())

    def test_site_is_served_only_during_a_run(self):
        import urllib.request
        with tempfile.TemporaryDirectory() as folder:
            task = Path(folder)
            self.assertEqual(bench.serve_site(task), (None, ""))
            (task / "site").mkdir()
            (task / "site" / "index.html").write_text("hello docs")
            server, url = bench.serve_site(task)
            try:
                self.assertTrue(url.startswith("http://127.0.0.1:"))
                self.assertEqual(urllib.request.urlopen(url + "/index.html", timeout=5).read(), b"hello docs")
            finally:
                server.shutdown()
                server.server_close()


class ArchiveTests(unittest.TestCase):
    def test_pack_is_reproducible_and_unpacks(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            src = root / "src"
            SuiteTests.make(None, src, "coding", "01-a")
            (src / "coding" / "tasks" / "01-a" / "verify").mkdir()
            check = src / "coding" / "tasks" / "01-a" / "verify" / "check.sh"
            check.write_text("exit 0")
            check.chmod(0o755)
            (src / "coding" / "tasks" / "01-a" / "__pycache__").mkdir()
            (src / "coding" / "tasks" / "01-a" / "__pycache__" / "x.pyc").write_bytes(b"junk")
            bench.pack(src, root / "a.tar.gz")
            bench.pack(src, root / "b.tar.gz")
            self.assertEqual((root / "a.tar.gz").read_bytes(), (root / "b.tar.gz").read_bytes())
            out = bench.unpack(root / "a.tar.gz", root / "suites")
            self.assertEqual([t.name for t in bench.all_tasks(out)], ["01-a"])
            self.assertTrue(os.access(out / "coding" / "tasks" / "01-a" / "verify" / "check.sh", os.X_OK))
            self.assertFalse((out / "coding" / "tasks" / "01-a" / "__pycache__").exists())
            # Unchanged archive: left alone (a stray file survives); a new one gets a new folder
            # and the old one (and the unhashed folder of older versions) goes.
            (out / "stray").write_text("x")
            (root / "suites").mkdir()
            self.assertEqual(bench.unpack(root / "a.tar.gz", root / "suites"), out)
            self.assertTrue((out / "stray").exists())
            self.assertFalse((root / "suites").exists())
            SuiteTests.make(None, src, "research", "r01")
            bench.pack(src, root / "a.tar.gz")
            new = bench.unpack(root / "a.tar.gz", root / "suites")
            self.assertNotEqual(new, out)
            self.assertFalse(out.exists())
            out = new
            self.assertEqual(bench.suite_names(out), ["coding", "research"])

    def test_concurrent_unpacks_all_see_every_task(self):
        """The plugin starts several run.py processes at once; none may see a half-replaced folder."""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            src = root / "src"
            SuiteTests.make(None, src, "coding", *[f"{i:02}-t" for i in range(30)])
            bench.pack(src, root / "a.tar.gz")
            code = (
                "import sys; sys.path.insert(0, sys.argv[1]); import run; from pathlib import Path; "
                "out = run.unpack(Path(sys.argv[2]), Path(sys.argv[3])); "
                "print(len([t for t in run.all_tasks(out) if (t / 'task.md').read_text()]))"
            )
            procs = [
                subprocess.Popen([sys.executable, "-c", code, str(bench.HERE), str(root / "a.tar.gz"), str(root / "suites")], stdout=subprocess.PIPE, text=True)
                for _ in range(6)
            ]
            self.assertEqual([p.communicate()[0].strip() for p in procs], ["30"] * 6)
            self.assertEqual([p.name for p in root.iterdir() if p.name.startswith(("suites", ".suites"))], [bench.unpack(root / "a.tar.gz", root / "suites").name])

    @unittest.skipUnless(bench.SOURCES.is_dir(), "needs a winfer checkout")
    def test_shipped_archive_matches_sources(self):
        import tarfile
        with tarfile.open(bench.ARCHIVE) as tar:
            shipped = {m.name: tar.extractfile(m).read() for m in tar.getmembers() if m.isfile()}
        sources = {
            p.relative_to(bench.SOURCES).as_posix(): p.read_bytes()
            for p in bench.SOURCES.rglob("*")
            if p.is_file() and "__pycache__" not in p.parts and p.name != ".DS_Store"
        }
        self.assertEqual(sorted(shipped), sorted(sources), "run `python3 run.py --pack`")
        self.assertEqual(shipped, sources, "run `python3 run.py --pack`")


class PrepareTests(unittest.TestCase):
    """Work folders behave the same under any Git line-ending config (Git for Windows sets
    core.autocrlf=true), so untouched tests never count as modified."""

    def test_line_endings_are_kept_byte_for_byte(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "gitconfig").write_text("[core]\n\tautocrlf = true\n\tsafecrlf = warn\n")
            task = folder / "task"
            (task / "repo" / "tests").mkdir(parents=True)
            (task / "repo" / "tests" / "test_lf.py").write_bytes(b"a = 1\nb = 2\n")
            (task / "repo" / "tests" / "test_crlf.py").write_bytes(b"a = 1\r\nb = 2\r\n")
            work = folder / "work"
            with patch.dict(os.environ, {"GIT_CONFIG_GLOBAL": str(folder / "gitconfig")}):
                bench.prepare(task, work)
                time.sleep(1.1)  # past racy-git: the next diff reads the files again
                for f in (work / "tests").iterdir():
                    os.utime(f)
                git = lambda *a: subprocess.run(["git", *a], cwd=work, capture_output=True, text=True)
                diff = git("diff", "--quiet", "HEAD", "--", "tests")
                self.assertEqual((diff.returncode, diff.stderr), (0, ""))
                blob = subprocess.run(["git", "show", "HEAD:tests/test_crlf.py"], cwd=work, capture_output=True).stdout
                self.assertEqual(blob, b"a = 1\r\nb = 2\r\n")
                (work / "tests" / "test_crlf.py").write_bytes(b"a = 1\nb = 2\n")
                self.assertEqual(git("diff", "--quiet", "HEAD", "--", "tests").returncode, 1)
                self.assertIn("test_crlf.py", bench.changed_files(work))

    def test_windows_temp_is_the_long_path(self):
        bench.temp_dir.cache_clear()
        try:
            with patch.object(bench, "WINDOWS", True), patch.object(bench.tempfile, "gettempdir", return_value="C:/X/RICKYP~1/Temp"), \
                    patch.object(bench.os.path, "realpath", return_value="C:/X/rickyperlick/Temp"):
                self.assertEqual(bench.temp_env(), {"TEMP": "C:/X/rickyperlick/Temp", "TMP": "C:/X/rickyperlick/Temp"})
        finally:
            bench.temp_dir.cache_clear()


class ControlTests(unittest.TestCase):
    """The commands the plugin runs instead of POSIX shell lines (so Windows works too)."""

    def test_npm_shims(self):
        npm = '@ECHO off\r\nSETLOCAL\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & "%_prog%"  "%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js" %*\r\n'
        self.assertEqual(bench.node_shim_script(npm), os.path.join("node_modules", "@openai", "codex", "bin", "codex.js"))
        pnpm = '@"%~dp0\\node.exe" "%~dp0\\..\\cli\\bin\\run.js" %*'
        self.assertEqual(bench.node_shim_script(pnpm), os.path.join("..", "cli", "bin", "run.js"))
        self.assertIsNone(bench.node_shim_script("@echo off"))

    def test_windows_unix_tools_come_before_system32(self):
        # Git's bare usr\bin\sh.exe is on PATH; Git's bin\sh.exe wins, and Git\usr\bin (GNU
        # timeout, find, sort) goes ahead of System32 for the check scripts.
        with tempfile.TemporaryDirectory() as folder:
            git = Path(folder) / "Git"
            for f in ("cmd/git.exe", "bin/sh.exe", "usr/bin/sh.exe", "usr/bin/timeout.exe"):
                (git / f).parent.mkdir(parents=True, exist_ok=True)
                (git / f).touch()
            which = {"git": str(git / "cmd" / "git.exe"), "sh": str(git / "usr" / "bin" / "sh.exe")}
            bench.posix_sh.cache_clear()
            bench.unix_tool_dirs.cache_clear()
            try:
                with patch.object(bench, "WINDOWS", True), patch.object(bench.shutil, "which", side_effect=which.get), \
                        patch.dict(os.environ, {"PATH": "SYSTEM32"}, clear=True), \
                        patch.object(bench, "python_shims", return_value="SHIMS"):
                    self.assertEqual(Path(bench.posix_sh()), (git / "bin" / "sh.exe").resolve())
                    path = bench.script_env()["PATH"].split(os.pathsep)
                self.assertEqual(path, ["SHIMS", str((git / "usr" / "bin").resolve()), "SYSTEM32"])
            finally:
                bench.posix_sh.cache_clear()
                bench.unix_tool_dirs.cache_clear()

    def test_launcher_leaves_posix_argv_alone(self):
        if not bench.WINDOWS:
            self.assertEqual(bench.launcher(["codex", "exec", "a\nb"]), ["codex", "exec", "a\nb"])

    def test_without_option(self):
        argv = ["-H", "x", "--detach", "o.out", "--log", "l", "--detach=p"]
        self.assertEqual(bench.without_option(argv, "--detach"), ["-H", "x", "--log", "l"])

    def test_history_and_delete(self):
        with tempfile.TemporaryDirectory() as folder:
            d = Path(folder) / "data"
            self.assertEqual(bench.history(d), [])
            for i, name in enumerate(["old.jsonl", "new.jsonl", "old.out", "keep.txt"]):
                (d / name).write_text("")
                os.utime(d / name, (1000 + i, 1000 + i))
            self.assertEqual(bench.history(d), ["new.jsonl", "old.jsonl"])
            bench.delete([str(d / "old.jsonl"), str(d / "old.out"), str(d / "keep.txt"), str(d / "gone.usage.json")])
            self.assertEqual(sorted(p.name for p in d.iterdir()), ["keep.txt", "new.jsonl"])

    def test_list_writes_a_file(self):
        with tempfile.TemporaryDirectory() as folder:
            out = Path(folder) / "sub" / "catalog.json"
            subprocess.run([sys.executable, str(bench.HERE / "run.py"), "--list", str(out)], check=True)
            catalog = json.loads(out.read_text(encoding="utf-8"))
            self.assertTrue(catalog["tasks"] and catalog["harnesses"])

    def test_detach_alive_stop(self):
        with tempfile.TemporaryDirectory() as folder:
            out = Path(folder) / "run.out"
            script = Path(folder) / "sleeper.py"
            script.write_text("import time\nprint('up', flush=True)\ntime.sleep(60)\n")
            with patch.object(bench, "__file__", str(script)):
                pid = bench.detach([], out)
            self.assertTrue(bench.alive(pid))
            bench.stop(pid)
            for _ in range(50):
                if not bench.alive(pid):
                    break
                try:  # reap it: a zombie child still answers kill -0
                    os.waitpid(pid, os.WNOHANG)
                except (ChildProcessError, AttributeError):
                    pass
                time.sleep(0.1)
            self.assertFalse(bench.alive(pid))


if __name__ == "__main__":
    unittest.main()
