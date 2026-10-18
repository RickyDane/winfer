"""Generates an exported issue/PR tracker for the `skiff` project into argv[1] (deterministic)."""

import json
import os
import random
import sys

root = sys.argv[1]
rng = random.Random(303)


def write(path, data):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(data if isinstance(data, str) else json.dumps(data, indent=2) + "\n")


USERS = ["alice-m", "bkowalski", "chen.l", "dmitri", "erin-ops", "fatima", "gus", "hana", "ivo", "jpark"]
MAINT = ["alice-m", "dmitri"]


def c(author, at, body):
    return {"author": author, "created_at": at, "body": body}


issues, pulls = {}, {}

# ---- the story ----
issues[412] = {
    "number": 412, "title": "Multipart uploads larger than ~4 GiB produce corrupted objects",
    "author": "erin-ops", "state": "closed", "labels": ["bug", "upload", "data-loss"],
    "created_at": "2025-01-08T09:14:00Z", "closed_at": "2025-02-12T16:40:00Z",
    "body": "Uploading a 6 GiB file with `skiff put` succeeds but the stored object fails checksum verification. Files under 4 GiB are fine. Default config.",
    "comments": [
        c("chen.l", "2025-01-08T11:02:00Z", "Same here on 2.7.2. I downgraded to 2.5.2 and the same file uploads fine. Didn't try 2.6.x."),
        c("gus", "2025-01-08T15:30:00Z", "Probably #377 (parallel part uploads) - parts finishing out of order?"),
        c("bkowalski", "2025-01-09T08:12:00Z", "Workaround that seems to work for me: set `upload.concurrency = 1`."),
        c("erin-ops", "2025-01-09T10:45:00Z", "`upload.concurrency = 1` did not help here, still corrupted (just slower)."),
        c("alice-m", "2025-01-09T18:20:00Z", "Bisected: it is not #377. The first bad commit is the merge of #380 (compact part headers), which stores the part number in a single byte. With the default 16 MiB part size anything above 255 parts (~4 GiB) wraps around. Every release since that PR landed is affected."),
        c("hana", "2025-01-10T07:55:00Z", "So bigger parts avoid it? I set `upload.part_size_mb = 64` and my 10 GiB file is OK now."),
        c("alice-m", "2025-01-10T09:01:00Z", "Yes, keep every upload at 255 parts or fewer. 64 only covers objects up to ~16 GiB though. Recommended workaround until a fixed release: `upload.part_size_mb = 256` (covers up to ~63 GiB). Don't disable multipart, single-part PUTs are capped at 5 GiB by the server."),
        c("ivo", "2025-01-12T13:00:00Z", "I opened #415 with a fix (uint16 part numbers)."),
        c("dmitri", "2025-01-20T14:05:00Z", "Closing via #419."),
        c("fatima", "2025-01-24T10:12:00Z", "#419 broke resuming uploads that were started with an older version, it was reverted in #423. Reopening."),
        c("alice-m", "2025-02-12T16:41:00Z", "Fixed for real by #431 (uint32 part numbers plus migration of saved resume state). It will be in the next minor release; a backport to 2.7 is planned."),
    ],
    "events": [
        {"event": "closed", "at": "2025-01-20T14:05:00Z", "by": "pull/419"},
        {"event": "reopened", "at": "2025-01-24T10:12:00Z", "by": "fatima"},
        {"event": "closed", "at": "2025-02-12T16:40:00Z", "by": "pull/431"},
    ],
}
issues[427] = {
    "number": 427, "title": "6GB backup corrupted after upload", "author": "jpark", "state": "closed",
    "labels": ["duplicate"], "created_at": "2025-01-28T12:00:00Z", "closed_at": "2025-01-28T13:10:00Z",
    "body": "Upgraded to 2.7.4 and my nightly backup (6 GB) is corrupt again. It worked on 2.7.3!",
    "comments": [c("dmitri", "2025-01-28T13:10:00Z", "Duplicate of #412: the fix in 2.7.3 was reverted in 2.7.4. Use the workaround from #412.")],
    "events": [{"event": "closed", "at": "2025-01-28T13:10:00Z", "by": "dmitri"}],
}
pulls[377] = {"number": 377, "title": "Parallel part uploads", "author": "gus", "state": "merged", "base": "main", "merged_at": "2024-09-20T10:00:00Z",
              "body": "Uploads parts concurrently (`upload.concurrency`, default 4).", "comments": []}
pulls[380] = {"number": 380, "title": "Compact part headers", "author": "chen.l", "state": "merged", "base": "main", "merged_at": "2024-09-28T17:30:00Z",
              "body": "Shrinks the per-part header from 16 to 8 bytes. The part number field is now a u8.", "comments": [c("dmitri", "2024-09-27T09:00:00Z", "LGTM, nice saving.")]}
pulls[415] = {"number": 415, "title": "Fix part number overflow (uint16)", "author": "ivo", "state": "closed", "base": "main", "merged_at": None,
              "body": "Fixes #412 by widening the part number to u16.", "comments": [
                  c("alice-m", "2025-01-13T09:00:00Z", "Thanks! The protocol allows up to 10000 parts and we may raise that, so u16 is a stopgap; let's go straight to u32."),
                  c("ivo", "2025-01-15T19:00:00Z", "Closing in favour of #419.")]}
pulls[419] = {"number": 419, "title": "Use uint32 part numbers", "author": "alice-m", "state": "merged", "base": "main", "merged_at": "2025-01-20T14:05:00Z",
              "body": "Fixes #412. Supersedes #415.", "comments": [c("dmitri", "2025-01-20T13:50:00Z", "Approved.")]}
pulls[423] = {"number": 423, "title": "Revert \"Use uint32 part numbers\" (#419)", "author": "dmitri", "state": "merged", "base": "main", "merged_at": "2025-01-24T09:58:00Z",
              "body": "Reverts #419: uploads started before upgrading can no longer be resumed (saved resume state still has u8 part numbers). Reopens #412.", "comments": []}
pulls[431] = {"number": 431, "title": "uint32 part numbers with resume-state migration", "author": "alice-m", "state": "merged", "base": "main", "merged_at": "2025-02-12T16:40:00Z",
              "body": "Fixes #412. Re-applies #419 and migrates saved resume state on first use.", "comments": [c("fatima", "2025-02-11T10:00:00Z", "Tested resume across versions, works.")]}
pulls[437] = {"number": 437, "title": "[2.7] Backport #431: uint32 part numbers", "author": "dmitri", "state": "merged", "base": "release-2.7", "merged_at": "2025-02-27T11:00:00Z",
              "body": "Backport of #431 to the 2.7 branch.", "comments": []}
pulls[440] = {"number": 440, "title": "[2.6] Backport #431", "author": "ivo", "state": "closed", "base": "release-2.6", "merged_at": None,
              "body": "Backport to 2.6.", "comments": [c("alice-m", "2025-03-02T10:00:00Z", "2.6 is end-of-life, closing.")]}

# ---- noise ----
AREAS = ["download", "config", "auth", "progress bar", "listing", "retry", "proxy", "checksum", "resume", "delete", "copy", "presign", "logging", "windows paths", "shell completion"]
BUGS = ["crashes when {a} is used with an empty bucket", "{a}: wrong exit code on failure", "{a} ignores the --quiet flag", "slow {a} with many small files",
        "{a} fails behind a corporate proxy", "{a} prints a stack trace instead of an error", "{a} leaks file descriptors", "unicode filenames break {a}"]
FEATS = ["support {a} for S3-compatible endpoints", "add --dry-run to {a}", "config option for {a} timeout", "JSON output for {a}"]
num = 340
taken = set(issues) | set(pulls)
noise_pr_numbers = []
while num < 470:
    num += 1
    if num in taken:
        continue
    a = rng.choice(AREAS)
    at = f"2024-{rng.randint(8, 12):02d}-{rng.randint(1, 28):02d}T{rng.randint(0, 23):02d}:{rng.randint(0, 59):02d}:00Z" if num < 400 else f"2025-0{rng.randint(1, 3)}-{rng.randint(1, 28):02d}T{rng.randint(0, 23):02d}:{rng.randint(0, 59):02d}:00Z"
    if rng.random() < 0.55:
        bug = rng.random() < 0.7
        title = (rng.choice(BUGS) if bug else rng.choice(FEATS)).format(a=a)
        comments = [c(rng.choice(USERS), at, rng.choice(["+1", "Can reproduce on 2.7.x.", "Any update?", "Which version?", "Logs attached.", "Works for me on main."])) for _ in range(rng.randint(0, 4))]
        closed = rng.random() < 0.6
        issues[num] = {"number": num, "title": title[0].upper() + title[1:], "author": rng.choice(USERS), "state": "closed" if closed else "open",
                       "labels": ["bug" if bug else "enhancement", a.split()[0]], "created_at": at, "closed_at": at if closed else None,
                       "body": f"Steps: run `skiff {a.split()[0]}` ... Expected it to work.", "comments": comments, "events": []}
    else:
        merged = rng.random() < 0.75
        title = rng.choice(["Fix {a} edge case", "Refactor {a} module", "Speed up {a}", "Add tests for {a}", "Document {a}", "Bump dependencies"]).format(a=a)
        pulls[num] = {"number": num, "title": title, "author": rng.choice(USERS), "state": "merged" if merged else rng.choice(["closed", "open"]),
                      "base": "main", "merged_at": at if merged else None, "body": rng.choice(["", "Small change.", "See discussion in the linked issue."]), "comments": []}
        if merged:
            noise_pr_numbers.append(num)

for n, i in issues.items():
    write(f"export/issues/{n}.json", i)
for n, p in pulls.items():
    write(f"export/pulls/{n}.json", p)

# ---- releases: the ground truth for what shipped where ----
merged_noise = sorted(noise_pr_numbers, key=lambda n: pulls[n]["merged_at"])
REL = [
    ("2.5.2", "2024-09-01", "main", []),
    ("2.6.0", "2024-10-15", "main", [377, 380]),
    ("2.6.1", "2024-11-02", "release-2.6", []),
    ("2.7.0", "2024-12-02", "main", []),
    ("2.7.1", "2024-12-18", "release-2.7", []),
    ("2.7.2", "2025-01-06", "release-2.7", []),
    ("2.7.3", "2025-01-22", "release-2.7", [419]),
    ("2.7.4", "2025-01-27", "release-2.7", [423]),
    ("2.8.0", "2025-02-20", "main", [431]),
    ("2.7.5", "2025-03-04", "release-2.7", [437]),
    ("2.8.1", "2025-03-25", "release-2.8", []),
]
rels = []
for ver, date, branch, prs in REL:
    extra = [n for n in merged_noise if pulls[n]["merged_at"][:10] <= date and not any(n in r["pull_requests"] for r in rels)]
    extra = extra[: rng.randint(2, 6)]
    rels.append({"tag": f"v{ver}", "published_at": f"{date}T12:00:00Z", "target_branch": branch, "pull_requests": sorted(prs + extra)})
write("export/releases.json", {"note": "Generated from the release pipeline: every pull request whose changes are in each release.", "releases": rels})

write("docs/known-issues.md", """# Known issues

- **Large multipart uploads corrupted (#412)**: fixed in 2.7.3. Upgrade to 2.7.3 or later.
- **Progress bar flickers on Windows terminals**: cosmetic, no fix planned.
""")
write("README.md", """# skiff tracker export

Exported issues (`export/issues/`), pull requests (`export/pulls/`) and releases (`export/releases.json`)
of the skiff object-storage CLI, plus a few docs pages.
""")
