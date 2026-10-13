#!/usr/bin/env python3
"""Same latency probes as bench_cfg.py, but in-process with MLX (mlx-community/clef-flash-4bit + clef_mlx.py).

  ~/.cache/winfer-spike/venv-mlx/bin/python -I mlx_bench.py MODEL_DIR [--n 3] [--eval]

No HTTP server is started (clef_mlx.py serve has no auth). --eval also runs action_1 of the
two-stage stage-1 request over evals.jsonl for an accuracy cross-check.
"""
import argparse, json, os, statistics, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("model_dir")
    ap.add_argument("--n", type=int, default=3)
    ap.add_argument("--eval", action="store_true")
    a = ap.parse_args()
    sys.path.insert(0, HERE)
    import winfer_request as wr  # spike code (no server access needed)
    from bench_cfg import syn
    sys.path.append(a.model_dir)  # appended, so it cannot shadow stdlib/site modules
    import clef_mlx
    import mlx.core as mx

    t = time.perf_counter()
    model = clef_mlx.load(a.model_dir)
    load_s = time.perf_counter() - t
    out = {"label": "mlx_clef-flash-4bit", "load_s": round(load_s, 2), "mlx": mx.__version__}
    bodies = {
        "floor": {"model": "clef-flash", "state": "hi", "questions": {"q": {"type": "noul", "instructions": "Is this a greeting?"}}},
        "syn500": {"model": "clef-flash", **syn(14)}, "syn1k": {"model": "clef-flash", **syn(32)},
        "syn2k": {"model": "clef-flash", **syn(68)},
        "stage1_1k": wr.stage1("open two claudes and a codex"),
        "s_rich": wr.build("open two claudes and a codex", "s_rich"),
        "real_full": wr.build("open two claudes and a codex", "full"),
    }
    for k, body in bodies.items():
        model.systemone(body)  # warm this shape
        lat, toks = [], 0
        for _ in range(a.n):
            t = time.perf_counter()
            r = model.systemone(body)
            lat.append((time.perf_counter() - t) * 1000)
            toks = r["usage"]["input_tokens"]
        p50 = statistics.median(lat)
        out[k] = {"tokens": toks, "p50": round(p50, 1), "min": round(min(lat), 1), "max": round(max(lat), 1),
                  "tok_per_s": round(toks / (p50 / 1000), 1)}
        print(k, json.dumps(out[k]), flush=True)
        if k == "real_full":
            out["real_full_example_answers"] = {q: (v.get("choice"), round(v.get("confidence", v.get("noul", 0)), 3)) for q, v in r["answers"].items()}
    out["peak_memory_gb"] = round(mx.get_peak_memory() / 1e9, 2)
    if a.eval:
        cases = [json.loads(l) for l in open(os.path.join(HERE, "evals.jsonl")) if l.strip()]
        ok, lat, rows = 0, [], []
        for c in cases:
            t = time.perf_counter()
            r = model.systemone(wr.build(c["utterance"], "s_rich"))
            lat.append((time.perf_counter() - t) * 1000)
            a1 = r["answers"]["action_1"]
            good = a1["choice"] in c["action_1"]
            ok += good
            rows.append({"id": c["id"], "a1": a1["choice"], "conf": round(a1["confidence"], 3), "ok": good})
            print(("OK " if good else "XX ") + f"{c['id']:>2} {lat[-1]:6.0f}ms {a1['choice']}({a1['confidence']:.2f}) | {c['utterance']}", flush=True)
        out["s_rich_eval"] = {"n": len(cases), "action_1_acc": ok / len(cases), "p50_ms": statistics.median(lat),
                              "max_ms": max(lat), "rows": rows}
        print("s_rich action_1 acc", ok, "/", len(cases), flush=True)
    out["peak_memory_gb"] = round(mx.get_peak_memory() / 1e9, 2)
    os.makedirs(os.path.join(HERE, "results"), exist_ok=True)
    json.dump(out, open(os.path.join(HERE, "results", "mlx_bench.json"), "w"), indent=2)
    print(json.dumps({k: v for k, v in out.items() if k != "s_rich_eval"}), flush=True)


if __name__ == "__main__":
    main()
