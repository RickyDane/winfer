#!/usr/bin/env python3
"""Post-hoc analysis of results/eval_<size>.jsonl: action_2 gating rule and threshold table.
  python3 posthoc.py compact full twostage
"""
import json, os, sys
HERE = os.path.dirname(os.path.abspath(__file__))
for size in sys.argv[1:]:
    rows = [json.loads(l) for l in open(os.path.join(HERE, "results", f"eval_{size}.jsonl"))]
    print(f"== {size} n={len(rows)}")
    if "action_2" in rows[0]["got"] and size != "twostage":
        for t in (0.0, 0.2, 0.3, 0.4, 0.5):
            ok = multi_ok = n_multi = 0
            for r in rows:
                a1 = r["got"]["action_1"][0]
                a2, c2 = r["got"]["action_2"]
                pred = a2 if (a2 != a1 and a2 != "none" and c2 >= t) else "none"
                exp = (r["expected"]["action_2"] or ["none"])
                ok += pred in exp
                if r["expected"]["action_2"]:
                    n_multi += 1; multi_ok += pred in exp
            print(f"  action_2 gate (a2!=a1, conf>={t}): {ok}/{len(rows)} overall, {multi_ok}/{n_multi} on multi cases")
    print("  threshold on action_1 confidence: auto>=hi / confirm [lo,hi) / reject <lo  -> n(correct)")
    for hi, lo in ((0.9, 0.5), (0.8, 0.4), (0.7, 0.4), (0.6, 0.3), (0.5, 0.2), (0.4, 0.15)):
        b = {"auto": [0, 0], "confirm": [0, 0], "reject": [0, 0]}
        for r in rows:
            c = r["conf_1"]; k = "auto" if c >= hi else "confirm" if c >= lo else "reject"
            b[k][0] += 1; b[k][1] += r["ok"]["action_1"]
        print(f"   hi={hi} lo={lo}: " + "  ".join(f"{k} {v[0]}({v[1]} ok, {v[0]-v[1]} wrong)" for k, v in b.items()))
