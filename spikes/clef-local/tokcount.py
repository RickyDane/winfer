#!/usr/bin/env python3
"""Count clef /v1/systemone prompt tokens without running the model.

Re-renders tokenizer.chat_template.systemone (see REPORT.md) piece by piece the way
server-decision.cpp fill_task_joint() does (keys sorted, choice options sorted, noul true first),
tokenizes each piece with the server's /tokenize endpoint and returns the sum + a per-question breakdown.
"""
import json, os, sys, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import eval as ev  # noqa: E402

NOUL_T = "The proposition is true or the answer is yes."
NOUL_F = "The proposition is false or the answer is no."
_cache = {}


def ntok(s):
    if s in _cache:
        return _cache[s]
    req = urllib.request.Request(ev.URL + "/tokenize", data=json.dumps({"content": s}).encode(),
                                 headers={"Content-Type": "application/json", "Authorization": "Bearer " + ev.KEY})
    n = len(json.load(urllib.request.urlopen(req))["tokens"])
    _cache[s] = n
    return n


def j(v):
    def srt(x):
        if isinstance(x, dict):
            return {k: srt(x[k]) for k in sorted(x)}
        if isinstance(x, list):
            return [srt(i) for i in x]
        return x
    return json.dumps(srt(v), separators=(",", ":"), ensure_ascii=False)


def options(q):
    t = q["type"]
    if t == "choice":
        return sorted(q["criteria"].items())
    if t == "score":
        return [(str(i), d) for i, d in enumerate(q["criteria"])]
    c = q.get("criteria") or {}
    return [("true", c.get("true")), ("false", c.get("false"))]


def count(body):
    st = body["state"]
    pieces_head = ["<|im_start|>system\nRead the complete state and schema. Decide every field jointly. Each answer must be exactly one of that field's allowed options.<|im_end|>\n<|im_start|>user\nSTATE:\n",
                   st if isinstance(st, str) else j(st), "\n\nSCHEMA FIELDS:\n"]
    head = sum(ntok(p) for p in pieces_head)
    per_q = {}
    for i, (qid, q) in enumerate(body["questions"].items(), 1):
        n = ntok(f"\nFIELD {i}\nID: {qid}\nTYPE: {q['type']}\nINSTRUCTION: ")
        ins = q["instructions"]
        n += ntok(ins if isinstance(ins, str) else j(ins)) + ntok("\nALLOWED OPTIONS:\n")
        for k, (key, d) in enumerate(options(q), 1):
            if q["type"] == "noul" and d is None:
                d = NOUL_T if key == "true" else NOUL_F
            n += ntok(f"OPTION {k}: ") + ntok(j({"option_id": key} if d is None else {"description": d, "option_id": key})) + ntok("\n")
        n += ntok("END FIELD\n")
        per_q[qid] = n
    tail = ntok("\n<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\nJOINT SCHEMA DECISIONS:")
    return {"total": head + sum(per_q.values()) + tail, "state+system": head, "tail": tail, "per_question": per_q}


if __name__ == "__main__":
    import winfer_request as wr
    for size in sys.argv[1:] or ["compact"]:
        print(size, json.dumps(count(wr.build("open cloud code", size))))
