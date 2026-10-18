#!/usr/bin/env python3
"""Runs coding-agent harnesses on the tasks in suites/*/tasks/ and compares success, time and tokens.

Tasks are grouped in suites (coding, understanding, research, discipline). Each run gets a
fresh copy of the task's project (a git repo, so changes are visible), the harness works
headless with permissions off, then the task's hidden check decides pass/fail.

    python3 bench/run.py                                  # winfer-chatgpt vs codex, all tasks
    python3 bench/run.py -s understanding,research        # only these suites
    python3 bench/run.py -H winfer-chatgpt,codex,pi -t 01,05 -n 3
    python3 bench/run.py -H oracle                        # checks the checks (reference solutions)
    python3 bench/run.py -H codex -m codex=gpt-6-luna      # another model for one harness
    python3 bench/run.py -H codex,claude -e codex=high -e claude=high   # a reasoning effort
    python3 bench/run.py --list                           # harnesses and tasks as JSON (for the UI)
    python3 bench/run.py --pack                           # rebuild suites.tar.gz after editing tasks

The plugin drives everything through this script (so it works the same under cmd.exe on Windows):
    --list FILE, --history DIR, --delete PATH..., --detach OUT (start a run in the background and
    print its pid), --alive PID, --stop PID, --find LOG_NAME.
"""

import argparse
import gzip
import hashlib
import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from functools import lru_cache
from pathlib import Path

WINDOWS = os.name == "nt"
# Output goes to a file the UI shows; Windows' default code page can't encode ▸ and friends.
for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        _stream.reconfigure(encoding="utf-8", errors="replace")

HERE = Path(__file__).resolve().parent
# The tasks ship as one archive (the plugin store installs at most 500 files per plugin), built
# with --pack from the sources in a winfer checkout's plugins-src/harness-bench/suites.
ARCHIVE = HERE / "suites.tar.gz"
SOURCES = HERE.parents[2] / "plugins-src" / "harness-bench" / "suites"
SUITE_ORDER = ["coding", "understanding", "research", "discipline"]


def pack(src: Path, archive: Path) -> None:
    """Writes `src` to `archive` reproducibly: same files, same bytes."""
    with archive.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0, filename="") as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.USTAR_FORMAT) as tar:
            for p in sorted(src.rglob("*")):
                if not p.is_file() or "__pycache__" in p.parts or p.name == ".DS_Store":
                    continue
                info = tarfile.TarInfo(p.relative_to(src).as_posix())
                info.size = p.stat().st_size
                info.mode = 0o755 if os.access(p, os.X_OK) else 0o644
                with p.open("rb") as f:
                    tar.addfile(info, f)


def unpack(archive: Path, dest: Path) -> Path:
    """Extracts `archive` next to `dest`, into a folder named after the archive's hash
    (`suites-<hash>`), unless it's there already; returns that folder. Safe when several
    run.py processes start at once (the plugin runs a few together): each extracts to its own
    temporary folder and renames it into place, and a folder in use is never replaced."""
    digest = hashlib.sha1(archive.read_bytes()).hexdigest()[:12]
    final = dest.parent / f"{dest.name}-{digest}"
    if not final.is_dir():
        tmp = Path(tempfile.mkdtemp(prefix=f".{dest.name}-", dir=dest.parent))
        with tarfile.open(archive) as tar:
            if hasattr(tarfile, "data_filter"):
                tar.extractall(tmp, filter="data")
            else:  # Python before 3.12 / 3.9.17: our own archive, but still refuse paths leaving tmp
                for m in tar.getmembers():
                    if m.name.startswith("/") or ".." in Path(m.name).parts or not (m.isfile() or m.isdir()):
                        sys.exit(f"{archive}: unexpected entry {m.name}")
                tar.extractall(tmp)
        try:
            tmp.rename(final)
        except OSError:  # another process got there first
            shutil.rmtree(tmp, ignore_errors=True)
            if not final.is_dir():
                raise
    # Earlier archives' folders (and the unhashed one older versions used), when nothing holds them.
    for old in dest.parent.glob(f"{dest.name}*"):
        if old.is_dir() and old != final and (old == dest or old.name.startswith(f"{dest.name}-")):
            shutil.rmtree(old, ignore_errors=True)
    for stamp in dest.parent.glob(f".{dest.name}.sha1"):
        stamp.unlink(missing_ok=True)
    return final


def find_suites() -> Path:
    if os.environ.get("BENCH_SUITES"):
        return Path(os.environ["BENCH_SUITES"])
    if SOURCES.is_dir():
        return SOURCES
    if ARCHIVE.exists():
        return unpack(ARCHIVE, HERE / "suites")
    return HERE / "suites"


@lru_cache(maxsize=None)
def suites_root() -> Path:
    """The task suites, found (and unpacked) on first use: the control commands don't need them."""
    return find_suites()
CATALOG = json.loads((HERE / "harnesses.json").read_text(encoding="utf-8"))
PRICES = CATALOG.get("_prices", {})


def winfer_bin() -> str:
    exe = ".exe" if WINDOWS else ""
    for c in [
        os.environ.get("WINFER_BIN"),
        # Run from a winfer checkout (plugins/harness-bench/bench).
        str(HERE.parents[2] / f"src-tauri/target/release/winfer{exe}"),
        str(HERE.parents[2] / f"src-tauri/target/debug/winfer{exe}"),
        "/Applications/winfer.app/Contents/MacOS/winfer",
        # The Windows installer's per-user and per-machine folders.
        *(str(Path(os.environ[v]) / "winfer" / "winfer.exe") for v in ("LOCALAPPDATA", "ProgramFiles") if WINDOWS and os.environ.get(v)),
    ]:
        if c and Path(c).exists():
            return str(Path(c).resolve())
    sys.exit("winfer binary not found; build it or set WINFER_BIN")


def git_roots() -> list:
    """Where Git for Windows may be installed."""
    roots = []
    git = shutil.which("git")
    if git:
        roots += list(Path(git).resolve().parents)[:3]  # Git\cmd\git.exe, Git\mingw64\bin\git.exe
    roots += [Path(os.environ[v]) / "Git" for v in ("ProgramFiles", "ProgramW6432", "LOCALAPPDATA") if os.environ.get(v)]
    roots += [Path(os.environ["LOCALAPPDATA"]) / "Programs" / "Git"] if os.environ.get("LOCALAPPDATA") else []
    return roots


@lru_cache(maxsize=None)
def posix_sh() -> str:
    """sh for the tasks' check and solution scripts. Windows has none of its own: use Git for
    Windows' (git is needed for the work folders anyway). Its bin\\sh.exe comes first, before an
    `sh` on PATH: that one is often Git's bare usr\\bin\\sh.exe, which doesn't put the Unix tools
    ahead of System32 the way bin\\sh.exe does."""
    if not WINDOWS:
        return shutil.which("sh") or "sh"
    for r in git_roots():
        if (r / "bin" / "sh.exe").exists():
            return str(r / "bin" / "sh.exe")
    return shutil.which("sh") or "sh"


@lru_cache(maxsize=None)
def unix_tool_dirs() -> list:
    """The folders with the Unix tools next to posix_sh() (Git\\usr\\bin), on Windows. The check
    scripts use `timeout`, `find` and `sort`, which System32 also has under the same names
    (timeout.exe rejects GNU arguments and exits 1), so these must come first on PATH."""
    sh = Path(posix_sh())
    dirs = [sh.parent, sh.parent.parent / "usr" / "bin"]  # usr\bin\sh.exe, or Git\bin\sh.exe
    return [str(d) for d in dirs if (d / "timeout.exe").exists() or (d / "sort.exe").exists()]


@lru_cache(maxsize=None)
def python_shims() -> str:
    """A folder with `python3` and `python` scripts that run this Python, for the check scripts
    on Windows, where `python3` is often missing or the Microsoft Store placeholder."""
    d = Path(tempfile.mkdtemp(prefix="bench-python-"))
    for name in ("python3", "python"):
        (d / name).write_text(f'#!/bin/sh\nexec "{Path(sys.executable).as_posix()}" "$@"\n', encoding="utf-8", newline="\n")
    return str(d)


@lru_cache(maxsize=None)
def temp_dir() -> str:
    """The temp folder for work folders, harnesses and checks. On Windows %TEMP% is often an 8.3
    short path (C:\\Users\\RICKYP~1\\...) that realpath() expands to the long one, so a path
    built from it and the same path resolved don't compare equal: use the long form."""
    t = tempfile.gettempdir()
    return os.path.realpath(t) if WINDOWS else t


def temp_env() -> dict:
    return {"TEMP": temp_dir(), "TMP": temp_dir()} if WINDOWS else {}


def script_env(**extra) -> dict:
    env = {**os.environ, **temp_env(), **extra}
    if WINDOWS:
        env["PATH"] = os.pathsep.join([python_shims(), *unix_tool_dirs(), env.get("PATH", "")])
    return env


def node_shim_script(text: str):
    """The script an npm/pnpm `.cmd` shim runs (`"%dp0%\\node_modules\\...\\cli.js" %*`), relative
    to the shim, or None."""
    line = next((l for l in reversed(text.splitlines()) if "%*" in l), None)
    if line is None:
        return None
    rels = []
    for part in line.split('"'):
        for prefix in ("%dp0%\\", "%~dp0\\", "%~dp0"):
            if part.startswith(prefix):
                rel = part[len(prefix):]
                if rel and rel.lower() != "node.exe":
                    rels.append(rel)
                break
    return rels[-1].replace("\\", os.sep) if rels else None


def launcher(argv):
    """How to start a harness CLI. On Windows npm installs CLIs as `.cmd` shims, which
    subprocess can't find by name and which would pass the multi-line prompt through cmd.exe:
    run the shim's script with Node (or its native program) directly."""
    if not WINDOWS or not argv:
        return argv
    path = shutil.which(argv[0])
    if not path:
        return argv
    shim = Path(path)
    if shim.suffix.lower() in (".cmd", ".bat"):
        rel = node_shim_script(shim.read_text(encoding="utf-8", errors="replace"))
        target = shim.parent / rel if rel else None
        if target and target.is_file():
            ext = target.suffix.lower()
            if ext in (".exe", ".com"):
                return [str(target), *argv[1:]]
            with target.open("rb") as f:
                first = f.readline(128)
            if ext in (".js", ".cjs", ".mjs") or (first.startswith(b"#!") and b"node" in first):
                node = shim.parent / "node.exe"
                node = str(node) if node.exists() else shutil.which("node")
                if node:
                    return [node, str(target), *argv[1:]]
    return [path, *argv[1:]]


def fingerprint(root: Path) -> str:
    """Hash of every file under `root` (paths and contents)."""
    h = hashlib.sha1()
    for p in sorted(root.rglob("*")):
        if p.is_file() and "__pycache__" not in p.parts:
            h.update(str(p.relative_to(root)).encode())
            h.update(p.read_bytes())
    return h.hexdigest()


def suite_names(root: Path = None):
    """Suite folder names under `root`, known suites first in their usual order."""
    root = root or suites_root()
    found = [p.name for p in root.iterdir() if (p / "tasks").is_dir()] if root.is_dir() else []
    rank = lambda s: (SUITE_ORDER.index(s) if s in SUITE_ORDER else len(SUITE_ORDER), s)
    return sorted(found, key=rank)


def suite_info(name: str, root: Path = None) -> dict:
    root = root or suites_root()
    f = root / name / "suite.json"
    info = json.loads(f.read_text(encoding="utf-8")) if f.exists() else {}
    return {"id": name, "label": info.get("label", name.capitalize()), "description": info.get("description", "")}


def all_tasks(root: Path = None, suites=None):
    """Task folders (those with a task.md) under `root`, grouped by suite then sorted by name."""
    root = root or suites_root()
    out = []
    for s in suite_names(root):
        if suites and s not in suites:
            continue
        out += sorted(t for t in (root / s / "tasks").iterdir() if (t / "task.md").is_file())
    return out


def suite_of(task: Path) -> str:
    """suites/<suite>/tasks/<task> → <suite>."""
    return task.parent.parent.name


def prepare(task: Path, work: Path) -> None:
    if (task / "repo").exists():
        shutil.copytree(task / "repo", work, dirs_exist_ok=True)
    if (task / "setup.py").exists():
        subprocess.run([sys.executable, str(task / "setup.py"), str(work)], check=True)
    git = lambda *a: subprocess.run(["git", *a], cwd=work, check=True, capture_output=True)
    git("init", "-q")
    # Keep files byte for byte whatever the user's Git config says. With core.autocrlf=true (Git
    # for Windows' default) a file can be committed with one line ending and compared with the
    # other, and the checks' `git diff --quiet HEAD -- tests` then reports untouched tests as
    # modified. Repo-local config and info/attributes win over global/system config.
    git("config", "core.autocrlf", "false")
    git("config", "core.safecrlf", "false")
    (work / ".git" / "info").mkdir(parents=True, exist_ok=True)
    (work / ".git" / "info" / "attributes").write_text("* -text\n", encoding="utf-8")
    git("add", "-A")
    git("-c", "user.name=bench", "-c", "user.email=bench@local", "commit", "-q", "--allow-empty", "-m", "start")


# ---- output parsers: (input tokens, cached input tokens, output tokens, tool calls, cost) ----

def lines(out: str):
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("{"):
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                pass


def parse_winfer(out):
    usage, tools = {}, 0
    for e in lines(out):
        if e.get("type") == "usage":
            usage = e
        elif e.get("type") == "tool_start":
            tools += 1
    # Bridges report totals; the ChatGPT backend reports running totals too.
    return usage.get("input", 0), usage.get("cached", 0), usage.get("output", 0), tools, usage.get("cost")


def parse_claude(out):
    i = c = o = tools = 0
    cost = None
    for e in lines(out):
        if e.get("type") == "assistant":
            tools += sum(1 for b in e.get("message", {}).get("content", []) if b.get("type") == "tool_use")
        if e.get("type") == "result":
            u = e.get("usage", {})
            c = u.get("cache_read_input_tokens", 0)
            i = u.get("input_tokens", 0) + c + u.get("cache_creation_input_tokens", 0)
            o = u.get("output_tokens", 0)
            cost = e.get("total_cost_usd")
    return i, c, o, tools, cost


def parse_codex(out):
    i = c = o = tools = 0
    for e in lines(out):
        if e.get("type") == "turn.completed":
            u = e.get("usage", {})
            i += u.get("input_tokens", 0)
            c += u.get("cached_input_tokens", 0)
            o += u.get("output_tokens", 0)
        if e.get("type") == "item.completed" and e.get("item", {}).get("type") not in ("agent_message", "reasoning"):
            tools += 1
    return i, c, o, tools, None


def parse_pi(out):
    i = c = o = tools = 0
    cost = 0.0
    for e in lines(out):
        m = e.get("message", {})
        if e.get("type") == "message_end" and m.get("role") == "assistant":
            u = m.get("usage", {})
            i += u.get("input", 0) + u.get("cacheRead", 0) + u.get("cacheWrite", 0)
            c += u.get("cacheRead", 0)
            o += u.get("output", 0)
            cost += (u.get("cost") or {}).get("total", 0) or 0
            tools += sum(1 for b in m.get("content", []) if b.get("type") in ("toolCall", "tool_use"))
    return i, c, o, tools, cost or None


def parse_opencode(out):
    i = c = o = tools = 0
    cost = 0.0
    for e in lines(out):
        if e.get("type") == "tool_use":
            tools += 1
        if e.get("type") == "step_finish":
            p = e.get("part", {})
            t = p.get("tokens", {})
            cache = t.get("cache", {})
            # `input` excludes what came from the prompt cache.
            i += t.get("input", 0) + cache.get("read", 0) + cache.get("write", 0)
            c += cache.get("read", 0)
            o += t.get("output", 0) + t.get("reasoning", 0)
            cost += p.get("cost") or 0
    return i, c, o, tools, cost or None


PARSERS = {"winfer": parse_winfer, "claude": parse_claude, "codex": parse_codex, "pi": parse_pi, "opencode": parse_opencode, "none": lambda out: (0, 0, 0, 0, None)}


def estimate_cost(model, tokens_in, cached, tokens_out):
    """Standard API token estimate; aggregate usage cannot recover per-request surcharges."""
    price = PRICES.get(model) or PRICES.get(model.rsplit("/", 1)[-1])
    if price is None or not (tokens_in > 0 or tokens_out > 0):
        return None
    cached = min(tokens_in, max(0, cached))
    return ((tokens_in - cached) * price["input"] + cached * price["cached"] + tokens_out * price["output"]) / 1_000_000


def serve_site(task: Path):
    """Serves the task's `site/` folder (a fake docs site for research tasks) on a free local
    port for the length of one run. Returns (server or None, base URL or "")."""
    root = task / "site"
    if not root.is_dir():
        return None, ""

    class Quiet(SimpleHTTPRequestHandler):
        def __init__(self, *a, **kw):
            super().__init__(*a, directory=str(root), **kw)

        def log_message(self, *a):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Quiet)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_address[1]}"


def harness_argv(spec, model: str, effort: str, prompt: str):
    """The harness command line: placeholders filled in, plus the effort when one is picked.
    Effort goes in before {prompt} (some CLIs stop reading options at the prompt), or into the
    model name when the spec has a model template (e.g. OpenCode's provider/model#variant)."""
    how = spec.get("effort") or {}
    argv = list(spec["argv"])
    model_arg = model
    if effort:
        if how.get("model"):
            model_arg = how["model"].replace("{model}", model).replace("{effort}", effort)
        elif how.get("argv"):
            at = argv.index("{prompt}") if "{prompt}" in argv else len(argv)
            argv[at:at] = [a.replace("{effort}", effort) for a in how["argv"]]
    return [a.replace("{prompt}", prompt).replace("{model}", model_arg) for a in argv]


def changed_files(work: Path) -> str:
    """The tracked files that differ from the start commit, for a check's "was modified" message."""
    r = subprocess.run(["git", "-c", "core.quotepath=off", "diff", "--stat=80", "HEAD"], cwd=work,
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    return "\n" + "\n".join(r.stdout.strip().splitlines()[-6:]) if r.stdout.strip() else ""


def run_one(name, spec, task: Path, timeout: int, keep: bool, model: str = "", effort: str = ""):
    work = Path(tempfile.mkdtemp(prefix=f"bench-{task.name}-{name}-", dir=temp_dir()))
    prepare(task, work)
    server, site = serve_site(task)
    prompt = (task / "task.md").read_text(encoding="utf-8").strip().replace("{site}", site)
    started = time.time()
    out, err, code, timed_out = "", "", 0, False
    if spec["parser"] == "oracle":
        if (task / "solution").exists():
            shutil.copytree(task / "solution", work, dirs_exist_ok=True)
        if (task / "solution.sh").exists():
            subprocess.run([posix_sh(), str(task / "solution.sh")], cwd=work, check=True, env=script_env())
    else:
        model = model or spec.get("model", "")
        argv = harness_argv(spec, model, effort, prompt)
        argv = launcher([winfer_bin() if a == "{winfer}" else a for a in argv])
        try:
            # Some CLIs take their project folder from $PWD rather than the real working directory.
            env = {**os.environ, **temp_env(), "PWD": str(work)}
            r = subprocess.run(argv, cwd=work, env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout, stdin=subprocess.DEVNULL)
            out, err, code = r.stdout, r.stderr, r.returncode
        except FileNotFoundError as e:
            out, code = json.dumps({"type": "error", "text": f"harness not installed: {e.filename}"}), 127
        except subprocess.TimeoutExpired as e:
            out, timed_out = (e.stdout or b"").decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or ""), True
    seconds = time.time() - started
    if server:
        server.shutdown()
        server.server_close()
    env = script_env(VERIFY_DIR=str(task / "verify"))
    check = subprocess.run([posix_sh(), str(task / "verify/check.sh")], cwd=work, capture_output=True, text=True, encoding="utf-8", errors="replace", env=env)
    why = (check.stdout + check.stderr).strip()[-400:] if check.returncode else ""
    if "was modified" in why:
        why += changed_files(work)
    i, c, o, tools, cost = PARSERS.get(spec["parser"], PARSERS["none"])(out)
    cost_estimated = False
    if cost is None:
        cost = estimate_cost(model or spec.get("model", ""), i, c, o)
        cost_estimated = cost is not None
    error = None if timed_out else harness_error(spec, out, err, code, i, tools)
    result = {
        "task": task.name,
        "suite": suite_of(task),
        "harness": name,
        "model": model or spec.get("model", ""),
        "effort": effort,
        "pass": check.returncode == 0 and not timed_out and not error,
        # The harness itself failed (crashed, rate-limited, not logged in) without doing the task.
        "error": error,
        "started": round(started, 2),
        "seconds": round(seconds, 1),
        "input": i,
        "cached": c,
        "output": o,
        "tools": tools,
        "cost": cost,
        "cost_estimated": cost_estimated,
        "timed_out": timed_out,
        "why": why,
        "dir": str(work) if keep else None,
    }
    (work / ".bench-output.jsonl").write_text(out, encoding="utf-8")
    if not keep:
        shutil.rmtree(work, ignore_errors=True)
    return result


def harness_error(spec, out, err, code, tokens_in, tools):
    """Why the harness failed to work on the task at all, or None. A harness that ran and got
    the task wrong is a failure, not an error."""
    if spec["parser"] in ("oracle", "none") and code == 0:
        return None
    did_nothing = tokens_in == 0 and tools == 0
    messages = []
    for e in lines(out):
        if e.get("type") in ("error", "exited", "turn.failed", "result") and e.get("is_error", True):
            detail = e.get("error")
            m = e.get("text") or e.get("message") or (detail.get("message") if isinstance(detail, dict) else detail)
            if isinstance(m, str) and m.strip():
                messages.append(m.strip())
    crashed = code not in (0, None)
    if did_nothing and (crashed or (messages and spec["parser"] != "none")):
        tail = messages[-1] if messages else (err.strip().splitlines() or [f"exit code {code}"])[-1]
        return tail[-300:]
    return None


def fmt_tokens(n):
    return f"{n / 1000:.1f}k" if n >= 1000 else str(int(n))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("-H", "--harnesses", default="winfer-chatgpt,codex")
    p.add_argument("-s", "--suites", default="all", help="comma-separated suites, e.g. coding,research")
    p.add_argument("-t", "--tasks", default="all", help="comma-separated task prefixes, e.g. 01,03,u02")
    p.add_argument("-n", "--runs", type=int, default=1)
    p.add_argument("--timeout", type=int, default=900)
    p.add_argument("--keep", action="store_true", help="keep work directories for inspection")
    p.add_argument("--out", default=str(HERE / "results"))
    p.add_argument("-m", "--model", action="append", default=[], help="harness=model, e.g. codex=gpt-6-luna")
    p.add_argument("-e", "--effort", action="append", default=[], help="harness=level, a reasoning/thinking effort, e.g. codex=high (levels in harnesses.json)")
    p.add_argument("--log", help="write per-run results to this JSONL file")
    p.add_argument("--status", help="keep a JSON progress file up to date (for the UI)")
    p.add_argument("--list", nargs="?", const="-", metavar="FILE", help="print (or write to FILE) harnesses and tasks as JSON and exit")
    p.add_argument("--history", metavar="DIR", help="print the saved runs (*.jsonl) in DIR, newest first, and exit")
    p.add_argument("--delete", nargs="+", metavar="PATH", help="delete saved run files (.jsonl, .out, .usage.json) and exit")
    p.add_argument("--detach", metavar="OUT", help="start this run in the background with its output in OUT; print its pid")
    p.add_argument("--alive", type=int, metavar="PID", help="print yes if the process is running, else no")
    p.add_argument("--stop", type=int, metavar="PID", help="stop a background run and the agents it started")
    p.add_argument("--find", metavar="LOG_NAME", help="print the pid of a background run writing LOG_NAME (POSIX only)")
    p.add_argument("--winfer-bin", help="the winfer program (same as $WINFER_BIN)")
    p.add_argument("--pack", action="store_true", help=f"rebuild {ARCHIVE.name} from {SOURCES} and exit")
    p.add_argument("--retries", type=int, default=1, help="retry a run whose harness errored (rate limit, crash) this many times")
    p.add_argument("--retry-wait", type=int, default=30, help="seconds to wait before retrying an errored run")
    p.add_argument(
        "-p",
        "--parallel",
        choices=["harness", "all", "none"],
        default="harness",
        help="harness: every harness at once, each running its tasks in turn (default); all: every run at once; none: one at a time",
    )
    args = p.parse_args()

    harnesses = {k: v for k, v in CATALOG.items() if not k.startswith("_")}
    if args.winfer_bin:
        os.environ["WINFER_BIN"] = args.winfer_bin
    if args.history:
        print("\n".join(history(Path(args.history))))
        return
    if args.delete:
        delete(args.delete)
        return
    if args.alive is not None:
        print("yes" if alive(args.alive) else "no")
        return
    if args.stop is not None:
        stop(args.stop)
        return
    if args.find:
        print(find_runner(args.find) or "")
        return
    if args.list:
        text = json.dumps(listing(harnesses))
        if args.list == "-":
            print(text)
        else:
            Path(args.list).parent.mkdir(parents=True, exist_ok=True)
            Path(args.list).write_text(text, encoding="utf-8")
        return
    if args.pack:
        if not SOURCES.is_dir():
            sys.exit(f"no task sources at {SOURCES}; --pack runs in a winfer checkout")
        pack(SOURCES, ARCHIVE)
        print(f"{ARCHIVE}: {len(all_tasks(SOURCES))} tasks, {ARCHIVE.stat().st_size // 1024} KB")
        return
    models = dict(m.split("=", 1) for m in args.model if "=" in m)
    names = [h.strip() for h in args.harnesses.split(",")]
    for n in names:
        if n not in harnesses:
            sys.exit(f"unknown harness {n}; see harnesses.json")
    efforts = {k: v for k, v in (e.split("=", 1) for e in args.effort if "=" in e) if v and v != "default"}
    for n in efforts:
        if n in names and not harnesses.get(n, {}).get("effort"):
            sys.exit(f"{n} has no effort setting; see harnesses.json")
    if args.detach:
        if args.status:
            Path(args.status).unlink(missing_ok=True)
        print(detach(without_option(sys.argv[1:], "--detach"), Path(args.detach)))
        return
    # Harnesses get copies made from a snapshot taken now, so one that writes outside its work
    # folder (into the task sources) can't hand later runs a changed or pre-solved project.
    suites = None
    if args.suites != "all":
        suites = [s.strip() for s in args.suites.split(",") if s.strip()]
        for s in suites:
            if s not in suite_names():
                sys.exit(f"unknown suite {s}; have {', '.join(suite_names())}")
    SUITES = suites_root()
    snapshot = Path(tempfile.mkdtemp(prefix="bench-tasks-"))
    shutil.copytree(SUITES, snapshot / "suites", ignore=shutil.ignore_patterns("__pycache__"))
    original = fingerprint(SUITES)
    tasks = all_tasks(snapshot / "suites", suites)
    if args.tasks != "all":
        wanted = args.tasks.split(",")
        tasks = [t for t in tasks if any(t.name.startswith(w) for w in wanted)]

    os.makedirs(args.out, exist_ok=True)
    log = Path(args.log) if args.log else Path(args.out) / f"{time.strftime('%Y%m%d-%H%M%S')}.jsonl"
    log.parent.mkdir(parents=True, exist_ok=True)
    results = []
    total = len(tasks) * len(names) * args.runs
    lock = threading.Lock()
    running = {}  # job id -> what it's doing, for the status file

    def progress(**kw):
        if args.status:
            Path(args.status).write_text(json.dumps({"total": total, "done": len(results), "log": str(log), "pid": os.getpid(), **kw}), encoding="utf-8")

    def job(task, name, k):
        key = (task.name, name, k)
        with lock:
            running[key] = {"task": task.name, "harness": name, "run": k + 1, "since": time.time()}
            progress(running=list(running.values()))
            print(f"▸ {task.name} · {name} · run {k + 1}/{args.runs}", flush=True)
        for attempt in range(args.retries + 1):
            r = run_one(name, harnesses[name], task, args.timeout, args.keep, models.get(name, ""), efforts.get(name, ""))
            r["attempts"] = attempt + 1
            with lock:
                if fingerprint(SUITES) != original:
                    # It worked on the bench's own task files instead of its copy: undo that and
                    # don't count the run.
                    shutil.rmtree(SUITES)
                    shutil.copytree(snapshot / "suites", SUITES)
                    r["pass"] = False
                    r["error"] = f"{name} changed the bench's task files instead of its work folder (restored; run not counted)"
                    print(f"  ERROR {r['error']}", flush=True)
            if r["error"] and "task files" in r["error"]:
                break
            if not r.get("error") or attempt == args.retries:
                break
            with lock:
                print(f"  ERROR {task.name} · {name}: {r['error'][:120]} (retrying in {args.retry_wait}s)", flush=True)
            time.sleep(args.retry_wait)
        with lock:
            running.pop(key, None)
            results.append(r)
            with log.open("a", encoding="utf-8") as f:
                f.write(json.dumps(r) + "\n")
            progress(running=list(running.values()))
            status = "PASS" if r["pass"] else "ERROR" if r.get("error") else "FAIL"
            print(f"  {status}  {task.name} · {name}  {r['seconds']}s  in {fmt_tokens(r['input'])} (cached {fmt_tokens(r['cached'])})  out {fmt_tokens(r['output'])}  tools {r['tools']}", flush=True)
            if r["why"]:
                print("    " + r["why"].replace("\n", "\n    "), flush=True)

    jobs = [(task, name, k) for task in tasks for name in names for k in range(args.runs)]
    if args.parallel == "none":
        for j in jobs:
            job(*j)
    elif args.parallel == "all":
        with ThreadPoolExecutor(max_workers=len(jobs) or 1) as pool:
            list(pool.map(lambda j: job(*j), jobs))
    else:
        # One lane per harness: harnesses run side by side, each works through its tasks in turn
        # (so no harness competes with itself for its rate limit).
        lane = lambda name: [job(*j) for j in jobs if j[1] == name]
        with ThreadPoolExecutor(max_workers=len(names) or 1) as pool:
            list(pool.map(lane, names))

    # Summary per harness (medians over runs and tasks).
    print("\n| harness | pass | median time | median input | median uncached input | median output | tools |")
    print("|---|---|---|---|---|---|---|")
    for name in names:
        rs = [r for r in results if r["harness"] == name]
        if not rs:
            continue
        med = lambda k: statistics.median(r[k] for r in rs)
        uncached = statistics.median(r["input"] - r["cached"] for r in rs)
        passed = sum(r["pass"] for r in rs)
        print(f"| {name}{' · ' + efforts[name] if name in efforts else ''} | {passed}/{len(rs)} | {med('seconds'):.0f}s | {fmt_tokens(med('input'))} | {fmt_tokens(uncached)} | {fmt_tokens(med('output'))} | {med('tools'):.0f} |")
    ran = [s for s in suite_names(snapshot / "suites") if any(r["suite"] == s for r in results)]
    if len(ran) > 1:
        print("\n| harness | " + " | ".join(ran) + " |")
        print("|---|" + "---|" * len(ran))
        for name in names:
            cells = []
            for s in ran:
                rs = [r for r in results if r["harness"] == name and r["suite"] == s]
                cells.append(f"{sum(r['pass'] for r in rs)}/{len(rs)}" if rs else "—")
            print(f"| {name} | " + " | ".join(cells) + " |")
    print(f"\nPer-run results: {log}")
    shutil.rmtree(snapshot, ignore_errors=True)
    progress(running=[], finished=time.time())


# ---- control commands for the plugin (no POSIX shell needed) ----

def history(folder: Path):
    """Saved runs in `folder`, newest first."""
    folder.mkdir(parents=True, exist_ok=True)
    return [p.name for p in sorted(folder.glob("*.jsonl"), key=lambda p: p.stat().st_mtime, reverse=True)]


def delete(paths):
    """Deletes saved run files; anything else is left alone."""
    for p in map(Path, paths):
        if p.name.endswith((".jsonl", ".out", ".usage.json")):
            p.unlink(missing_ok=True)


def without_option(argv, name):
    """`argv` without `name VALUE` / `name=VALUE`."""
    out, skip = [], False
    for a in argv:
        if skip:
            skip = False
        elif a == name:
            skip = True
        elif not a.startswith(name + "="):
            out.append(a)
    return out


def detach(argv, out: Path) -> int:
    """Starts run.py with `argv` in the background, in its own process group (so stop() can end
    the agents too), with its output in `out`; returns its pid. Nothing of the caller's is
    inherited, so the caller's output pipe closes when it returns."""
    out.parent.mkdir(parents=True, exist_ok=True)
    kw = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW} if WINDOWS else {"start_new_session": True}
    with out.open("wb") as f:
        p = subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), *argv],
            cwd=HERE, stdin=subprocess.DEVNULL, stdout=f, stderr=subprocess.STDOUT, **kw,
        )
    return p.pid


def alive(pid: int) -> bool:
    if WINDOWS:
        import ctypes

        k = ctypes.windll.kernel32
        h = k.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if not h:
            return False
        code = ctypes.c_ulong()
        ok = k.GetExitCodeProcess(h, ctypes.byref(code))
        k.CloseHandle(h)
        return bool(ok) and code.value == 259  # STILL_ACTIVE
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def stop(pid: int) -> None:
    """Stops a background run and the agents it started."""
    if WINDOWS:
        subprocess.run(["taskkill", "/T", "/F", "/PID", str(pid)], capture_output=True)
        return
    try:
        group = os.getpgid(pid)
        if group != os.getpgid(0):  # never our own group (the app's, for an old-style run)
            os.killpg(group, signal.SIGTERM)
    except OSError:
        pass
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        pass


def find_runner(log_name: str):
    """The pid of a run writing `log_name`, for status files from before they held the pid."""
    if WINDOWS or not shutil.which("pgrep"):
        return None
    pattern = "run\\.py.*" + log_name.replace(".", "\\.")
    r = subprocess.run(["pgrep", "-f", pattern], capture_output=True, text=True)
    pids = [int(x) for x in r.stdout.split() if x.isdigit() and int(x) != os.getpid()]
    return pids[0] if pids else None


def listing(harnesses):
    """Harnesses (with whether their CLI is installed) and tasks, for the UI."""
    def installed(spec):
        argv = spec.get("argv") or []
        if not argv:
            return True
        # winfer code · Claude / Codex drive the claude and codex CLIs, so those must be there too.
        if any(shutil.which(c) is None for c in spec.get("requires", [])):
            return False
        if argv[0] == "{winfer}":
            try:
                return bool(winfer_bin())
            except SystemExit:
                return False
        return shutil.which(argv[0]) is not None

    tasks = []
    for t in all_tasks():
        prompt = (t / "task.md").read_text(encoding="utf-8").strip()
        tasks.append({"id": t.name, "suite": suite_of(t), "prompt": prompt, "hash": hashlib.sha1(prompt.encode()).hexdigest()[:8]})
    return {
        "suites": [suite_info(s) for s in suite_names()],
        "prices": PRICES,
        "pricing": CATALOG.get("_pricing", {}),
        "harnesses": [
            {"id": k, "label": v.get("label", k), "model": v.get("model", ""), "models": v.get("models", []), "efforts": (v.get("effort") or {}).get("levels", []), "installed": installed(v)}
            for k, v in harnesses.items()
        ],
        "tasks": tasks,
    }


if __name__ == "__main__":
    main()
