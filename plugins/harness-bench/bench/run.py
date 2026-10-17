#!/usr/bin/env python3
"""Runs coding-agent harnesses on the tasks in tasks/ and compares success, time and tokens.

Each run gets a fresh copy of the task's project (a git repo, so changes are visible), the
harness works headless with permissions off, then the task's hidden check decides pass/fail.

    python3 bench/run.py                                  # winfer-chatgpt vs codex, all tasks
    python3 bench/run.py -H winfer-chatgpt,codex,pi -t 01,05 -n 3
    python3 bench/run.py -H oracle                        # checks the checks (reference solutions)
    python3 bench/run.py -H codex -m codex=gpt-6-luna      # another model for one harness
    python3 bench/run.py --list                           # harnesses and tasks as JSON (for the UI)
"""

import argparse
import hashlib
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
TASKS = HERE / "tasks"


def winfer_bin() -> str:
    for c in [
        os.environ.get("WINFER_BIN"),
        # Run from a winfer checkout (plugins/harness-bench/bench).
        str(HERE.parents[2] / "src-tauri/target/release/winfer"),
        str(HERE.parents[2] / "src-tauri/target/debug/winfer"),
        "/Applications/winfer.app/Contents/MacOS/winfer",
    ]:
        if c and Path(c).exists():
            return str(Path(c).resolve())
    sys.exit("winfer binary not found; build it or set WINFER_BIN")


def fingerprint(root: Path) -> str:
    """Hash of every file under `root` (paths and contents)."""
    h = hashlib.sha1()
    for p in sorted(root.rglob("*")):
        if p.is_file() and "__pycache__" not in p.parts:
            h.update(str(p.relative_to(root)).encode())
            h.update(p.read_bytes())
    return h.hexdigest()


def prepare(task: Path, work: Path) -> None:
    if (task / "repo").exists():
        shutil.copytree(task / "repo", work, dirs_exist_ok=True)
    if (task / "setup.py").exists():
        subprocess.run([sys.executable, str(task / "setup.py"), str(work)], check=True)
    git = lambda *a: subprocess.run(["git", *a], cwd=work, check=True, capture_output=True)
    git("init", "-q")
    git("add", "-A")
    git("-c", "user.name=bench", "-c", "user.email=bench@local", "commit", "-qm", "start")


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


def run_one(name, spec, task: Path, timeout: int, keep: bool, model: str = ""):
    work = Path(tempfile.mkdtemp(prefix=f"bench-{task.name}-{name}-"))
    prepare(task, work)
    prompt = (task / "task.md").read_text().strip()
    started = time.time()
    out, err, code, timed_out = "", "", 0, False
    if spec["parser"] == "oracle":
        if (task / "solution").exists():
            shutil.copytree(task / "solution", work, dirs_exist_ok=True)
        if (task / "solution.sh").exists():
            subprocess.run(["sh", str(task / "solution.sh")], cwd=work, check=True)
    else:
        model = model or spec.get("model", "")
        argv = [a.replace("{prompt}", prompt).replace("{model}", model) for a in spec["argv"]]
        argv = [winfer_bin() if a == "{winfer}" else a for a in argv]
        try:
            # Some CLIs take their project folder from $PWD rather than the real working directory.
            env = {**os.environ, "PWD": str(work)}
            r = subprocess.run(argv, cwd=work, env=env, capture_output=True, text=True, timeout=timeout, stdin=subprocess.DEVNULL)
            out, err, code = r.stdout, r.stderr, r.returncode
        except FileNotFoundError as e:
            out, code = json.dumps({"type": "error", "text": f"harness not installed: {e.filename}"}), 127
        except subprocess.TimeoutExpired as e:
            out, timed_out = (e.stdout or b"").decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or ""), True
    seconds = time.time() - started
    env = {**os.environ, "VERIFY_DIR": str(task / "verify")}
    check = subprocess.run(["sh", str(task / "verify/check.sh")], cwd=work, capture_output=True, text=True, env=env)
    i, c, o, tools, cost = PARSERS.get(spec["parser"], PARSERS["none"])(out)
    error = None if timed_out else harness_error(spec, out, err, code, i, tools)
    result = {
        "task": task.name,
        "harness": name,
        "model": model or spec.get("model", ""),
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
        "timed_out": timed_out,
        "why": (check.stdout + check.stderr).strip()[-400:] if check.returncode else "",
        "dir": str(work) if keep else None,
    }
    (work / ".bench-output.jsonl").write_text(out)
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
    p.add_argument("-t", "--tasks", default="all", help="comma-separated task prefixes, e.g. 01,03")
    p.add_argument("-n", "--runs", type=int, default=1)
    p.add_argument("--timeout", type=int, default=900)
    p.add_argument("--keep", action="store_true", help="keep work directories for inspection")
    p.add_argument("--out", default=str(HERE / "results"))
    p.add_argument("-m", "--model", action="append", default=[], help="harness=model, e.g. codex=gpt-6-luna")
    p.add_argument("--log", help="write per-run results to this JSONL file")
    p.add_argument("--status", help="keep a JSON progress file up to date (for the UI)")
    p.add_argument("--list", action="store_true", help="print harnesses and tasks as JSON and exit")
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

    harnesses = {k: v for k, v in json.loads((HERE / "harnesses.json").read_text()).items() if not k.startswith("_")}
    if args.list:
        print(json.dumps(listing(harnesses)))
        return
    models = dict(m.split("=", 1) for m in args.model if "=" in m)
    names = [h.strip() for h in args.harnesses.split(",")]
    for n in names:
        if n not in harnesses:
            sys.exit(f"unknown harness {n}; see harnesses.json")
    # Harnesses get copies made from a snapshot taken now, so one that writes outside its work
    # folder (into the task sources) can't hand later runs a changed or pre-solved project.
    snapshot = Path(tempfile.mkdtemp(prefix="bench-tasks-"))
    shutil.copytree(TASKS, snapshot / "tasks", ignore=shutil.ignore_patterns("__pycache__"))
    original = fingerprint(TASKS)
    tasks = sorted(t for t in (snapshot / "tasks").iterdir() if t.is_dir())
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
            Path(args.status).write_text(json.dumps({"total": total, "done": len(results), "log": str(log), **kw}))

    def job(task, name, k):
        key = (task.name, name, k)
        with lock:
            running[key] = {"task": task.name, "harness": name, "run": k + 1, "since": time.time()}
            progress(running=list(running.values()))
            print(f"▸ {task.name} · {name} · run {k + 1}/{args.runs}", flush=True)
        for attempt in range(args.retries + 1):
            r = run_one(name, harnesses[name], task, args.timeout, args.keep, models.get(name, ""))
            r["attempts"] = attempt + 1
            with lock:
                if fingerprint(TASKS) != original:
                    # It worked on the bench's own task files instead of its copy: undo that and
                    # don't count the run.
                    shutil.rmtree(TASKS)
                    shutil.copytree(snapshot / "tasks", TASKS)
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
            with log.open("a") as f:
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
        print(f"| {name} | {passed}/{len(rs)} | {med('seconds'):.0f}s | {fmt_tokens(med('input'))} | {fmt_tokens(uncached)} | {fmt_tokens(med('output'))} | {med('tools'):.0f} |")
    print(f"\nPer-run results: {log}")
    shutil.rmtree(snapshot, ignore_errors=True)
    progress(running=[], finished=time.time())


def listing(harnesses):
    """Harnesses (with whether their CLI is installed) and tasks, for the UI."""
    def installed(spec):
        argv = spec.get("argv") or []
        if not argv:
            return True
        if argv[0] == "{winfer}":
            try:
                return bool(winfer_bin())
            except SystemExit:
                return False
        return shutil.which(argv[0]) is not None

    tasks = []
    for t in sorted(p for p in TASKS.iterdir() if p.is_dir()):
        prompt = (t / "task.md").read_text().strip()
        tasks.append({"id": t.name, "prompt": prompt, "hash": hashlib.sha1(prompt.encode()).hexdigest()[:8]})
    return {
        "harnesses": [
            {"id": k, "label": v.get("label", k), "model": v.get("model", ""), "models": v.get("models", []), "installed": installed(v)}
            for k, v in harnesses.items()
        ],
        "tasks": tasks,
    }


if __name__ == "__main__":
    main()
