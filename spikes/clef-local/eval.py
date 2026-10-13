#!/usr/bin/env python3
"""winfer M0.5 spike: Clef-flash /v1/systemone evals + latency/caching benches.

Usage (server must be running, see run_server.sh):
  python3 eval.py eval  [--size full|s|xs|large] [--limit N]
  python3 eval.py bench [--n 10]          # latency vs input tokens (xs, s, full, large)
  python3 eval.py cache [--n 10]          # transcript-first vs transcript-last string state + /metrics cache counters
Writes results/*.json(l) next to this file.
"""
import argparse, json, os, statistics, sys, time, urllib.request, urllib.error

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import winfer_request as wr  # noqa: E402

URL = os.environ.get("CLEF_URL", "http://127.0.0.1:8899")
KEY = os.environ.get("API_KEY") or open(os.path.join(HERE, ".api_key")).read().strip()
OUT = os.path.join(HERE, "results")
PROVIDERS = [p["id"] for p in wr.PROVIDERS]


def post(body):
    req = urllib.request.Request(URL + "/v1/systemone", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json", "Authorization": "Bearer " + KEY})
    t = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            res = json.load(r)
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"HTTP {e.code}: {e.read().decode()[:500]}")
    return (time.perf_counter() - t) * 1000.0, res


def metrics():
    req = urllib.request.Request(URL + "/metrics", headers={"Authorization": "Bearer " + KEY})
    vals = {}
    for line in urllib.request.urlopen(req).read().decode().splitlines():
        if line.startswith("llamacpp:"):
            k, v = line.split(" ")
            vals[k] = float(v)
    return vals


def pct(xs, p):
    xs = sorted(xs)
    if not xs:
        return float("nan")
    k = (len(xs) - 1) * p
    lo, hi = int(k), min(int(k) + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def argmax(d):
    return max(d, key=d.get)


def check_case(case, ans, size):
    """returns dict of per-field correctness (None = not checked)"""
    r = {}
    a1 = ans["action_1"]["choice"]
    r["action_1"] = a1 in case["action_1"]
    if "action_2" in ans:
        r["action_2"] = ans["action_2"]["choice"] in case.get("action_2", ["none"])
    args = case.get("args", {})
    checks = {}
    if "counts" in args:
        if any(k.startswith("spawn_panes.count.") for k in ans):
            exp = {p: args["counts"].get(p, 0) for p in PROVIDERS}
            got = {p: int(ans[f"spawn_panes.count.{p}"]["choice"]) for p in PROVIDERS}
            checks["counts"] = exp == got
        else:
            checks["counts"] = False  # not asked because the action was missed
        if "spawn_panes.worktree" in ans:
            checks["worktree"] = (ans["spawn_panes.worktree"]["noul"] > 0.5) == bool(args.get("worktree", False))
        elif "worktree" in args:
            checks["worktree"] = False
    if "target_pane" in args and "target_pane" in ans:
        checks["target_pane"] = ans["target_pane"]["choice"] in args["target_pane"]
    if "resize_amount" in args:
        checks["resize_amount"] = "resize_amount" in ans and int(argmax(ans["resize_amount"]["probabilities"])) in args["resize_amount"]
    if "workspace" in args:
        checks["workspace"] = "workspace" in ans and ans["workspace"]["choice"] == args["workspace"]
    r["arg_checks"] = checks
    r["args"] = all(checks.values()) if checks else None
    return r


def rss_kb():
    try:
        pid = open(os.path.join(HERE, "server.pid")).read().strip()
        return int(os.popen(f"ps -o rss= -p {pid}").read().strip() or 0)
    except Exception:
        return None


def summarize_answers(ans):
    out = {}
    for k, v in ans.items():
        if v["type"] == "choice":
            out[k] = (v["choice"], round(v["confidence"], 3))
        elif v["type"] == "score":
            out[k] = (argmax(v["probabilities"]), round(v["score"], 2), round(v["confidence"], 3))
        else:
            out[k] = round(v["noul"], 3)
    return out


def ask(utt, size):
    """returns (ms_total, answers, input_tokens_total, n_calls, ms_stage1)"""
    if size != "twostage":
        ms, res = post(wr.build(utt, size))
        return ms, res["answers"], res["usage"]["input_tokens"], 1, ms
    ms1, r1 = post(wr.stage1(utt))
    ans = dict(r1["answers"])
    toks = r1["usage"]["input_tokens"]
    a1 = ans["action_1"]["choice"]
    multi = ans["multi"]["noul"] > 0.5
    body2 = wr.stage2(utt, a1, multi)
    ms2 = 0.0
    if body2:
        ms2, r2 = post(body2)
        ans.update(r2["answers"])
        toks += r2["usage"]["input_tokens"]
    if not multi:
        ans["action_2"] = {"type": "choice", "choice": "none", "confidence": 1 - ans["multi"]["noul"], "probabilities": {}}
    return ms1 + ms2, ans, toks, 1 + bool(body2), ms1


def cmd_eval(a):
    cases = [json.loads(l) for l in open(os.path.join(HERE, "evals.jsonl")) if l.strip()]
    if a.limit:
        cases = cases[: a.limit]
    ask("warm up", a.size)  # warm
    rows = []
    for c in cases:
        ms, ans, toks, ncalls, ms1 = ask(c["utterance"], a.size)
        chk = check_case(c, ans, a.size)
        row = {"id": c["id"], "utterance": c["utterance"], "expected": {k: c.get(k) for k in ("action_1", "action_2", "args")},
               "got": summarize_answers(ans), "ok": chk, "ms": round(ms, 1), "ms_stage1": round(ms1, 1), "calls": ncalls,
               "input_tokens": toks, "tags": c.get("tags", []), "rss_kb": rss_kb(),
               "conf_1": ans["action_1"]["confidence"], "p_1": max(ans["action_1"]["probabilities"].values())}
        rows.append(row)
        mark = "OK " if chk["action_1"] else "XX "
        a2 = f" a2={ans['action_2']['choice']}({ans['action_2']['confidence']:.2f}){'' if chk.get('action_2', True) else '!'}" if "action_2" in ans else ""
        print(f"{mark}{c['id']:>2} {ms:7.0f}ms {toks:5d}t conf={row['conf_1']:.2f} a1={ans['action_1']['choice']:<18}{a2} args={chk['args']} {chk['arg_checks'] if chk['args'] is False else ''} | {c['utterance']}", flush=True)

    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, f"eval_{a.size}.jsonl"), "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")

    n = len(rows)
    a1 = sum(r["ok"]["action_1"] for r in rows)
    a2rows = [r for r in rows if "action_2" in r["ok"]]
    a2 = sum(r["ok"]["action_2"] for r in a2rows)
    a2multi = [r for r in a2rows if r["expected"]["action_2"]]
    argrows = [r for r in rows if r["ok"]["args"] is not None]
    argok = sum(r["ok"]["args"] for r in argrows)
    per_arg = {}
    for r in argrows:
        for k, v in r["ok"]["arg_checks"].items():
            per_arg.setdefault(k, [0, 0])
            per_arg[k][0] += v
            per_arg[k][1] += 1
    lat = [r["ms"] for r in rows]
    conf_ok = [r["conf_1"] for r in rows if r["ok"]["action_1"]]
    conf_bad = [r["conf_1"] for r in rows if not r["ok"]["action_1"]]

    def buckets(hi, lo):
        out = {}
        for name, f in (("auto", lambda c: c >= hi), ("confirm", lambda c: lo <= c < hi), ("reject", lambda c: c < lo)):
            sel = [r for r in rows if f(r["conf_1"])]
            out[name] = {"n": len(sel), "correct": sum(r["ok"]["action_1"] for r in sel)}
        return out

    sweep = {f"{hi}/{lo}": buckets(hi, lo) for hi, lo in ((0.9, 0.5), (0.85, 0.5), (0.75, 0.45), (0.65, 0.4), (0.5, 0.3))}
    summary = {
        "size": a.size, "n": n,
        "input_tokens": {"min": min(r["input_tokens"] for r in rows), "max": max(r["input_tokens"] for r in rows)},
        "action_1_acc": a1 / n,
        "action_2_acc": (a2 / len(a2rows)) if a2rows else None,
        "action_2_acc_multi_only": (sum(r["ok"]["action_2"] for r in a2multi) / len(a2multi)) if a2multi else None,
        "args_acc": (argok / len(argrows)) if argrows else None, "args_n": len(argrows),
        "per_arg": {k: f"{v[0]}/{v[1]}" for k, v in per_arg.items()},
        "latency_ms": {"p50": pct(lat, .5), "p95": pct(lat, .95), "max": max(lat), "min": min(lat)},
        "latency_stage1_ms": {"p50": pct([r["ms_stage1"] for r in rows], .5), "p95": pct([r["ms_stage1"] for r in rows], .95)},
        "calls_mean": statistics.mean(r["calls"] for r in rows),
        "input_tokens_mean": statistics.mean(r["input_tokens"] for r in rows),
        "peak_rss_kb": max((r["rss_kb"] or 0) for r in rows),
        "conf_correct_all": sorted(round(c, 3) for c in conf_ok),
        "conf_correct": {"n": len(conf_ok), "min": min(conf_ok, default=None), "p10": pct(conf_ok, .1), "median": pct(conf_ok, .5)},
        "conf_incorrect": {"n": len(conf_bad), "values": sorted(round(c, 3) for c in conf_bad)},
        "threshold_sweep_auto/confirm_lo": sweep,
        "failures": [{"id": r["id"], "utt": r["utterance"], "a1": r["got"]["action_1"], "exp": r["expected"]["action_1"]} for r in rows if not r["ok"]["action_1"]],
    }
    json.dump(summary, open(os.path.join(OUT, f"summary_{a.size}.json"), "w"), indent=2)
    print(json.dumps(summary, indent=2))


BENCH_UTTS = ["open two claudes and a codex", "close the codex one", "make the claude one a lot bigger",
              "switch to bridgemind api", "reload the browser", "tell claude to run the tests", "undo that",
              "zoom in on two", "approve claude", "um yeah so anyway"]


def cmd_bench(a):
    out = {}
    for size in a.sizes.split(","):
        post(wr.build("warm up", size))
        lat, toks = [], []
        for i in range(a.n):
            ms, res = post(wr.build(BENCH_UTTS[i % len(BENCH_UTTS)], size))
            lat.append(ms)
            toks.append(res["usage"]["input_tokens"])
        out[size] = {"tokens_mean": statistics.mean(toks), "p50": pct(lat, .5), "p95": pct(lat, .95), "max": max(lat),
                     "ms_per_token": pct(lat, .5) / statistics.mean(toks)}
        print(size, json.dumps(out[size]), flush=True)
    os.makedirs(OUT, exist_ok=True)
    json.dump(out, open(os.path.join(OUT, "bench.json"), "w"), indent=2)


def cmd_cache(a):
    out = {}
    variants = [("object_state", dict(state_as_string=False)),
                ("string_transcript_first", dict(state_as_string=True, transcript_first=True)),
                ("string_transcript_last", dict(state_as_string=True, transcript_first=False)),
                ("string_transcript_last_questions_reversed", dict(state_as_string=True, transcript_first=False, rev=True))]

    def build(utt, kw):
        kw = dict(kw)
        rev = kw.pop("rev", False)
        b = wr.build(utt, a.size, **kw)
        if rev:
            b["questions"] = dict(reversed(list(b["questions"].items())))
        return b

    for name, kw in variants:
        post(build("warm up", kw))
        m0 = metrics()
        lat, toks = [], []
        for i in range(a.n):
            ms, res = post(build(BENCH_UTTS[i % len(BENCH_UTTS)], kw))
            lat.append(ms)
            toks.append(res["usage"]["input_tokens"])
        m1 = metrics()
        # identical request repeated (best case for any prefix cache)
        body = build("open two claudes and a codex", kw)
        rep = [post(body)[0] for _ in range(3)]
        m2 = metrics()
        out[name] = {
            "tokens_mean": statistics.mean(toks), "p50_varying_transcript": pct(lat, .5),
            "p50_identical_repeat": pct(rep, .5),
            "metrics_delta_varying": {k.split(":")[1]: m1.get(k, 0) - m0.get(k, 0) for k in m1 if "prompt_tokens" in k},
            "metrics_delta_identical": {k.split(":")[1]: m2.get(k, 0) - m1.get(k, 0) for k in m2 if "prompt_tokens" in k},
        }
        print(name, json.dumps(out[name]), flush=True)
    os.makedirs(OUT, exist_ok=True)
    json.dump(out, open(os.path.join(OUT, "cache.json"), "w"), indent=2)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    e = sub.add_parser("eval"); e.add_argument("--size", default="full"); e.add_argument("--limit", type=int)
    b = sub.add_parser("bench"); b.add_argument("--n", type=int, default=10); b.add_argument("--sizes", default="xs,s,full,large")
    c = sub.add_parser("cache"); c.add_argument("--n", type=int, default=10); c.add_argument("--size", default="s")
    a = ap.parse_args()
    {"eval": cmd_eval, "bench": cmd_bench, "cache": cmd_cache}[a.cmd](a)
