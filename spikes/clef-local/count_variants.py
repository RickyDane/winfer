#!/usr/bin/env python3
"""A/B the encoding of the per-provider spawn counts on the eval cases that expect counts.

  python3 count_variants.py        (server must be running)

digits     : current compact stage-2 questions, options "0".."4"
words      : options "zero".."four" with short descriptions (the head has a lexical option projection)
words_nopanes : same, but the state is only the transcript (existing codex/claude panes may bias towards 0)
"""
import json, os, statistics, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import eval as ev  # noqa: E402
import winfer_request as wr  # noqa: E402

WORDS = ["zero", "one", "two", "three", "four"]
WORD_DESC = {"zero": "not mentioned", "one": "one, e.g. 'a X' or just 'X'", "two": "two", "three": "three", "four": "four"}


def q_words():
    q = {}
    for p in wr.PROVIDERS:
        pid = p["id"]
        q[f"spawn_panes.count.{pid}"] = {
            "type": "choice",
            "instructions": f"How many NEW {wr.PROVIDER_HINT[pid]} panes does the user ask to open?",
            "criteria": dict(WORD_DESC),
        }
    q["spawn_panes.worktree"] = wr.compact_questions()["spawn_panes.worktree"]
    return q


def body(variant, utt):
    if variant == "digits":
        return wr.stage2(utt, "spawn_panes", False)
    state = wr.compact_state(utt) if variant == "words" else f"Transcript: {utt}"
    return {"model": "clef-flash", "state": state, "questions": q_words()}


def to_int(v):
    return WORDS.index(v) if v in WORDS else int(v)


def main():
    cases = [json.loads(l) for l in open(os.path.join(HERE, "evals.jsonl")) if l.strip()]
    cases = [c for c in cases if "counts" in c.get("args", {})]
    out = {}
    for variant in sys.argv[1:] or ["digits", "words", "words_nopanes"]:
        ok, lat, toks, rows = 0, [], [], []
        for c in cases:
            ms, res = ev.post(body(variant, c["utterance"]))
            a = res["answers"]
            got = {p["id"]: to_int(a[f"spawn_panes.count.{p['id']}"]["choice"]) for p in wr.PROVIDERS}
            exp = {p["id"]: c["args"]["counts"].get(p["id"], 0) for p in wr.PROVIDERS}
            good = got == exp
            ok += good
            lat.append(ms)
            toks.append(res["usage"]["input_tokens"])
            rows.append({"id": c["id"], "ok": good, "got": {k: v for k, v in got.items() if v}})
            print(variant, "OK" if good else "XX", c["id"], {k: v for k, v in got.items() if v}, "|", c["utterance"], flush=True)
        out[variant] = {"counts_acc": f"{ok}/{len(cases)}", "p50_ms": round(statistics.median(lat)), "tokens": round(statistics.mean(toks)), "rows": rows}
        print(variant, out[variant]["counts_acc"], out[variant]["p50_ms"], "ms", out[variant]["tokens"], "tok", flush=True)
    json.dump(out, open(os.path.join(ev.OUT, "count_variants.json"), "w"), indent=2)


if __name__ == "__main__":
    main()
