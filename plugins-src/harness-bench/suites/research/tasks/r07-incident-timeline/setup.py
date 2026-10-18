"""Generates logs, deploy records, a chat export and a postmortem draft for an incident into argv[1] (deterministic)."""

import calendar
import json
import os
import random
import sys
import time

root = sys.argv[1]
rng = random.Random(707)

DAY = "2026-03-14"
PDT = -7 * 3600  # America/Los_Angeles on 2026-03-14 (DST started 2026-03-08)


def write(path, text):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text)


def T(hms):
    return calendar.timegm(time.strptime(f"{DAY} {hms}", "%Y-%m-%d %H:%M:%S"))


def iso_z(t):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))


def iso_off(t):
    return time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime(t))


def local(t):
    return time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(t + PDT))


START, END = T("20:00:00"), T("23:30:00")
INFO = {
    "api-gateway": ["GET /v1/cart 200 {ms}ms", "POST /v1/checkout 200 {ms}ms", "GET /v1/search 200 {ms}ms", "GET /v1/account 200 {ms}ms", "POST /v1/login 200 {ms}ms"],
    "billing": ["invoice {id} rendered", "charge {id} captured", "webhook from psp acknowledged", "ledger batch {id} committed"],
    "checkout": ["order {id} created", "payment intent {id} confirmed", "cart {id} locked", "inventory reserved for order {id}"],
    "db-proxy": ["pool stats: in_use={n} idle={m}", "client connected from 10.4.{n}.{m}", "health check ok"],
    "notification-worker": ["sent email {id}", "sent push {id}", "batch {id} drained"],
    "auth": ["login ok user={id}", "token refreshed user={id}", "session expired user={id}"],
    "search": ["query ok took={ms}ms", "index refresh ok", "shard {n} healthy"],
}
INCIDENT = {
    "db-proxy": (T("21:43:01"), T("22:30:52"), ["acquire rejected: pool exhausted (max_pool=20, waiters={n})"]),
    "billing": (T("21:42:49"), T("22:31:50"), ["timeout acquiring DB connection from db-proxy after 5000ms", "could not acquire DB connection: pool exhausted"]),
    "checkout": (T("21:43:12"), T("22:31:05"), ["db-proxy: timeout acquiring connection (pool exhausted)", "order {id} failed: database unavailable (db-proxy pool exhausted)"]),
    "api-gateway": (T("21:43:40"), T("22:31:20"), ["upstream checkout returned 503 (db pool exhausted)", "upstream billing returned 503 (db pool exhausted)"]),
    "notification-worker": (T("21:44:30"), T("22:32:14"), ["db-proxy acquire timeout while loading template {id}"]),
}
UNRELATED = {
    "auth": [(T("21:30:00"), T("22:10:00"), "LDAP sync failed: connection refused (ldap-2.internal:636)")],
    "search": [(T("22:05:00"), T("22:20:00"), "shard 3 unavailable: disk watermark exceeded")],
    "api-gateway": [(T("22:05:10"), T("22:19:50"), "upstream search returned 502 (shard unavailable)")],
    "checkout": [(T("21:20:05"), T("21:20:40"), "redis pool exhausted (session cache), retrying")],
    "billing": [(T("20:58:00"), T("20:58:00"), "invoice render failed: template 'eu-vat-v3' missing")],
}


def fill(s):
    return s.format(ms=rng.randint(4, 900), id=rng.randint(10000, 99999), n=rng.randint(1, 40), m=rng.randint(1, 250))


def events(svc):
    ev = []
    t = START
    while t < END:
        t += rng.randint(8, 30)
        ev.append((t, "INFO", fill(rng.choice(INFO[svc]))))
    if svc in INCIDENT:
        a, b, msgs = INCIDENT[svc]
        ev.append((a, "ERROR", fill(msgs[0])))
        ev.append((b, "ERROR", fill(msgs[-1])))
        t = a
        while True:
            t += rng.randint(15, 70)
            if t >= b:
                break
            ev.append((t, "ERROR", fill(rng.choice(msgs))))
    for a, b, msg in UNRELATED.get(svc, []):
        t = a
        ev.append((a, "ERROR", msg))
        while True:
            t += rng.randint(40, 120)
            if t >= b:
                break
            ev.append((t, "ERROR", msg))
    for _ in range(rng.randint(3, 8)):
        ev.append((rng.randint(START, END), "WARN", rng.choice(["slow request", "retrying upstream call", "gc pause 180ms"])))
    if svc == "db-proxy":
        ev.append((T("21:41:32"), "INFO", "config reloaded: max_pool=20 (was 200)"))
        ev.append((T("21:42:40"), "WARN", "pool at capacity (20/20)"))
        ev.append((T("22:30:47"), "INFO", "config reloaded: max_pool=200 (was 20)"))
    ev.sort(key=lambda e: e[0])
    return ev


for svc in ["api-gateway", "db-proxy", "auth", "search"]:
    write(f"logs/{svc}.log", "".join(f"{iso_z(t)} {lvl:<5} {svc} {msg}\n" for t, lvl, msg in events(svc)))
write("logs/checkout.log", "".join(f"[{iso_off(t)}] [{lvl}] {msg}\n" for t, lvl, msg in events("checkout")))
write("logs/billing.log", "".join(f"{local(t)} {lvl} billing: {msg}\n" for t, lvl, msg in events("billing")))
write("logs/notification-worker.jsonl", "".join(json.dumps({"ts": t, "level": lvl.lower(), "svc": "notification-worker", "msg": msg}) + "\n" for t, lvl, msg in events("notification-worker")))

write("docs/logging.md", """# Log formats

| Service | File | Timestamp |
|---|---|---|
| api-gateway, db-proxy, auth, search | `logs/<service>.log` | ISO 8601, UTC (`Z`) |
| checkout | `logs/checkout.log` | ISO 8601 with offset |
| billing | `logs/billing.log` | local wall-clock time of the billing hosts, which run in `America/Los_Angeles` (no offset written) |
| notification-worker | `logs/notification-worker.jsonl` | `ts` = Unix epoch seconds |

`deploys.log` and `commits.txt` use UTC.
""")

C_BAD = "4c9a1f7e3b2d8a6c5f1e0d9b8a7c6e5f4d3c2b1a"
C_DECOY = "b7e41d02c9a8f7e6d5c4b3a2918f7e6d5c4b3a29"
C_PREV_DB = "0d2b6c35e8f1a9b7c3d2e4f6a8b0c1d3e5f7a9b2"
C_PREV_CO = "91e0aa4c7d3f2b8e6a5c9d1f0e2b4a6c8d0f1e3a"
commits = [
    ("e3f1c09a7b5d2e8f4a6c1b3d5e7f9a0b2c4d6e8f", "2026-03-13T17:02:11Z", "lena", "search: bump shard replica count", ["search/config.yaml"]),
    (C_PREV_DB, "2026-03-13T18:40:00Z", "mei", "db-proxy: add pool metrics", ["db-proxy/metrics.go"]),
    (C_PREV_CO, "2026-03-13T19:12:45Z", "raj", "checkout: log payment intent ids", ["checkout/payments.py"]),
    ("5a7c9e1b3d5f7a9c1e3b5d7f9a1c3e5b7d9f1a3c", "2026-03-14T16:20:33Z", "omar", "auth: rotate LDAP bind credentials", ["auth/ldap.yaml"]),
    (C_BAD, "2026-03-14T21:12:09Z", "tom", "db-proxy: tune pool settings for new instance type", ["db-proxy/config/prod.yaml"]),
    (C_DECOY, "2026-03-14T21:30:52Z", "raj", "checkout: new retry policy for payment provider", ["checkout/retry.py", "checkout/payments.py"]),
    ("7f2d4b6a8c0e1f3a5b7d9c1e3f5a7b9d1c3e5f7a", "2026-03-14T23:05:00Z", "mei", "db-proxy: guard against max_pool < 100 in prod", ["db-proxy/config/validate.go"]),
]
write("commits.txt", "".join(f"commit {h}\nAuthor: {a}\nDate:   {d}\n\n    {m}\n\n" + "".join(f"    M {f}\n" for f in fs) + "\n" for h, d, a, m, fs in commits))
write("diffs/4c9a1f7.diff", f"""commit {C_BAD}
--- a/db-proxy/config/prod.yaml
+++ b/db-proxy/config/prod.yaml
@@ -3,5 +3,5 @@ pool:
-  max_pool: 200
+  max_pool: 20
   idle_timeout: 300s
-  acquire_timeout: 10s
+  acquire_timeout: 5s
""")
write("diffs/b7e41d0.diff", f"""commit {C_DECOY}
--- a/checkout/retry.py
+++ b/checkout/retry.py
@@ -1,4 +1,6 @@
-RETRIES = 2
+RETRIES = 3
+BACKOFF = 0.4
""")
write("deploys.log", "\n".join([
    "2026-03-14T16:31:02Z deploy auth 5a7c9e1 by omar ok",
    "2026-03-14T21:41:30Z deploy db-proxy 4c9a1f7 by tom ok",
    "2026-03-14T21:47:00Z deploy checkout b7e41d0 by raj ok",
    "2026-03-14T21:56:10Z rollback checkout b7e41d0 -> 91e0aa4 by raj ok",
    "2026-03-14T22:30:45Z rollback db-proxy 4c9a1f7 -> 0d2b6c3 by mei ok",
    "2026-03-14T22:50:12Z deploy checkout b7e41d0 by raj ok",
    "2026-03-14T23:20:40Z deploy db-proxy 7f2d4b6 by mei ok",
]) + "\n")

chat = [
    ("21:48:15", "pagerbot", "ALERT checkout 5xx rate > 5% (api-gateway)"),
    ("21:48:52", "ana", "looking. lots of 503s from checkout and billing at the gateway"),
    ("21:49:30", "raj", "I deployed b7e41d0 to checkout at 21:47, new retry policy. Could be that, rolling back."),
    ("21:57:02", "raj", "checkout rolled back to 91e0aa4. Still seeing 503s :("),
    ("22:02:40", "omar", "auth has LDAP sync errors since 21:30 too, that's my credential rotation, unrelated to checkout, logins work from cache"),
    ("22:06:15", "lena", "search shard 3 is out of disk, gateway shows search 502s. separate issue, fixing"),
    ("22:14:03", "mei", "db-proxy is rejecting acquires: pool exhausted with max_pool=20. That started ~21:43, before the checkout deploy."),
    ("22:16:40", "mei", "4c9a1f7 (deployed 21:41) changed max_pool 200 -> 20. Pretty sure it was meant to be 120. Rolling db-proxy back to 0d2b6c3."),
    ("22:31:30", "mei", "db-proxy rolled back, pool back to 200"),
    ("22:34:10", "ana", "gateway clean. billing and the notification worker drained their backlog a bit later. calling it resolved"),
    ("22:52:00", "raj", "re-deployed b7e41d0 to checkout, no errors. it was never the problem"),
    ("23:22:05", "mei", "7f2d4b6 adds a guard so prod can't go below 100 connections"),
]
write("chat/incident-2026-03-14.json", json.dumps({"channel": "#inc-checkout-503", "messages": [{"ts": T(h), "user": u, "text": x} for h, u, x in chat]}, indent=2) + "\n")

write("postmortem-draft.md", """# Postmortem (DRAFT): checkout 503s, 2026-03-14

Author: ana (draft, not reviewed)

## Summary

Between **14:42 UTC** and **22:31 UTC** customers saw errors at checkout.

## Root cause

Commit **b7e41d0** (checkout retry policy) increased load on the database and exhausted the
connection pool. The alert fired one minute after it was deployed.

## Affected services

api-gateway, checkout, billing, auth

## Timeline

- 21:47 b7e41d0 deployed to checkout
- 21:48 alert
- 21:56 checkout rolled back
- 22:30 db-proxy rolled back
- 22:31 last error

## Action items

- Load-test retry policy changes.
""")
write("README.md", "# Incident 2026-03-14 material\n\n`logs/`, `deploys.log`, `commits.txt`, `diffs/`, `chat/`, `postmortem-draft.md`, `docs/logging.md`.\n")
