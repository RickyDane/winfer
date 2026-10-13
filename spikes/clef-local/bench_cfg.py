#!/usr/bin/env python3
"""Latency vs input size for the currently running server config.

  python3 bench_cfg.py LABEL [--n 5]

Requests:
  floor     minimal noul question on "hi" (~140 tok)
  syn500/syn1k/syn2k  one 5-option choice question, state = padded app log text (~500/1k/2k tok)
  real_*    the winfer request at sizes compact / full (see winfer_request.py)
Appends one JSON line per config to results/bench_cfg.jsonl. Also records server-side
prompt seconds from /metrics to separate HTTP/JSON overhead from model compute.
"""
import argparse, json, os, statistics, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import eval as ev  # noqa: E402
import winfer_request as wr  # noqa: E402

LINE = "pane p{i} (provider claude) printed: ran the unit tests for module {i}, all green, waiting for the next instruction. "


def syn(n_lines):
    state = "".join(LINE.format(i=i) for i in range(n_lines))
    return {"state": "Transcript: open two claudes\nLog: " + state,
            "questions": {"a": {"type": "choice", "instructions": "Which command does the transcript ask for?",
                                "criteria": {"spawn_panes": None, "close_pane": None, "focus_pane": None, "none": None, "zoom_pane": None}}}}


def run(body, n):
    ev.post(body)  # warm this shape
    m0 = ev.metrics()
    lat, toks = [], 0
    for _ in range(n):
        ms, res = ev.post(body)
        lat.append(ms)
        toks = res["usage"]["input_tokens"]
    m1 = ev.metrics()
    srv_s = m1.get("llamacpp:prompt_seconds_total", 0) - m0.get("llamacpp:prompt_seconds_total", 0)
    return {"tokens": toks, "p50": round(ev.pct(lat, .5), 1), "min": round(min(lat), 1), "max": round(max(lat), 1),
            "server_prompt_ms_mean": round(srv_s * 1000 / n, 1) if srv_s else None,
            "tok_per_s": round(toks / (ev.pct(lat, .5) / 1000), 1)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("label")
    ap.add_argument("--n", type=int, default=5)
    ap.add_argument("--only", default="floor,syn500,syn1k,syn2k,real_compact,real_full")
    a = ap.parse_args()
    bodies = {
        "floor": {"state": "hi", "questions": {"q": {"type": "noul", "instructions": "Is this a greeting?"}}},
        "syn500": syn(14), "syn1k": syn(32), "syn2k": syn(68),
        "real_compact": wr.build("open two claudes and a codex", "compact"),
        "real_full": wr.build("open two claudes and a codex", "full"),
    }
    out = {"label": a.label, "ts": time.strftime("%H:%M:%S")}
    for k in a.only.split(","):
        try:
            out[k] = run(bodies[k], a.n)
        except Exception as e:  # keep the exact error
            out[k] = {"error": str(e)[:400]}
        print(a.label, k, json.dumps(out[k]), flush=True)
    os.makedirs(ev.OUT, exist_ok=True)
    with open(os.path.join(ev.OUT, "bench_cfg.jsonl"), "a") as f:
        f.write(json.dumps(out) + "\n")


if __name__ == "__main__":
    main()
