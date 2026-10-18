"""Generates logs/app.log (80k lines, deterministic) into argv[1]."""

import os
import random
import sys

root = sys.argv[1]
rng = random.Random(42)
os.makedirs(os.path.join(root, "logs"), exist_ok=True)
codes = ["E1001", "E1002", "E2040", "E3100", "E4004", "E5000", "E5001", "E7777"]
weights = [30, 8, 22, 5, 26, 12, 3, 1]
services = ["api", "worker", "auth", "billing", "search"]
with open(os.path.join(root, "logs", "app.log"), "w") as f:
    for i in range(80_000):
        ts = f"2026-10-{1 + i // 4000:02d}T{(i // 160) % 24:02d}:{(i // 3) % 60:02d}:{i % 60:02d}Z"
        svc = rng.choice(services)
        r = rng.random()
        if r < 0.0005:
            f.write("### truncated line\n")
        elif r < 0.12:
            code = rng.choices(codes, weights)[0]
            f.write(f"{ts} ERROR [{svc}] code={code} request failed id={rng.randrange(10**6)}\n")
        elif r < 0.20:
            # Warnings mention codes too, heavily skewed to E5000 to catch naive greps.
            code = rng.choice(["E5000", "E5000", "E5000", "E1002"])
            f.write(f"{ts} WARN [{svc}] retrying after code={code}\n")
        else:
            f.write(f"{ts} INFO [{svc}] handled request in {rng.randrange(1, 900)}ms\n")
