"""Generates docs for three fictional embedded key-value stores into argv[1] (deterministic)."""

import os
import random
import sys

root = sys.argv[1]
rng = random.Random(808)


def write(path, text):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text.lstrip("\n"))


NOISE = ["Fixed a crash when the data directory is on a network filesystem.", "Faster range scans ({n}% in our benchmark).",
         "Improved error messages for corrupted files.", "Reduced memory use of the write buffer.", "Fixed iterator leak on early break.",
         "Documentation fixes.", "Added `compact()` progress callback.", "Fixed a deadlock when closing during compaction.",
         "Type hints are now complete.", "Fixed `__repr__` of closed databases.", "Smaller wheels.", "Faster startup with large logs."]


def changelog(name, releases):
    out = f"# {name} changelog\n\nThis file is the authoritative history of {name} releases.\n\n"
    for ver, date, items in releases:
        its = list(items) + [rng.choice(NOISE).format(n=rng.randint(5, 40)) for _ in range(rng.randint(1, 4))]
        rng.shuffle(its)
        out += f"## {ver} ({date})\n\n" + "".join(f"- {i}\n" for i in its) + "\n"
    return out


MIT = "MIT License\n\nCopyright (c) {y} {who}\n\nPermission is hereby granted, free of charge, to any person obtaining a copy of this software ...\n"
APACHE = "                                 Apache License\n                           Version 2.0, January 2004\n                        http://www.apache.org/licenses/\n ...\n"
BSL = "Business Source License 1.1\n\nLicensor: Stash Labs, Inc.\nLicensed Work: stashdb 5.0 and later\nAdditional Use Grant: You may use the Licensed Work in production only for non-commercial purposes.\nChange Date: 2030-02-01\nChange License: Apache License, Version 2.0\n"

# ---------------- stashdb: fails C1 (relicensed to BSL in 5.0.0) ----------------
write("libs/stashdb/README.md", """
# stashdb

[![license: MIT](https://img.shields.io/badge/license-MIT-green)]

A fast embedded key-value store for Python with a native core.

- ACID transactions across any number of keys: `with db.transaction() as tx: ...`
- asyncio API in `stashdb.asyncio`
- Wheels for Linux (x86_64, aarch64), macOS and Windows
- Python 3.9 - 3.13

Latest release: 5.1.0
""")
write("libs/stashdb/LICENSE", BSL)
write("libs/stashdb/CHANGELOG.md", changelog("stashdb", [
    ("5.1.0", "2026-08-02", ["Python 3.13 wheels."]),
    ("5.0.0", "2026-02-01", ["**License change:** stashdb is now licensed under the Business Source License 1.1 (see LICENSE). Releases before 5.0.0 remain MIT.", "New storage format (automatic upgrade)."]),
    ("4.6.2", "2025-10-20", []),
    ("4.6.0", "2025-07-08", ["manylinux aarch64 wheels."]),
    ("4.5.0", "2025-03-30", ["`stashdb.asyncio` is no longer experimental."]),
    ("4.0.0", "2024-06-11", ["Multi-key transactions with serializable isolation.", "Dropped Python 3.8."]),
]))
write("libs/stashdb/docs/install.md", "# Installing stashdb\n\n```\npip install stashdb\n```\n\nPre-built wheels exist for CPython 3.9-3.13 on manylinux x86_64 and aarch64, macOS (arm64, x86_64) and Windows x86_64. No compiler needed.\n")
write("libs/stashdb/docs/transactions.md", "# Transactions\n\n```python\nwith db.transaction() as tx:\n    tx.put(b'a', b'1')\n    tx.delete(b'b')\n```\n\nTransactions are ACID and may touch any number of keys (serializable isolation).\n")
write("libs/stashdb/docs/asyncio.md", "# asyncio\n\n```python\nfrom stashdb.asyncio import open_db\ndb = await open_db('data')\nawait db.put(b'k', b'v')\n```\n")
write("libs/stashdb/docs/faq.md", "# FAQ\n\n**Is stashdb free for commercial use?** Yes, it's MIT licensed. *(written for 4.x)*\n\n**Does it support Windows?** Yes.\n")

# ---------------- kvelt: fails C2 (dropped Python 3.9 in 3.0.0) ----------------
write("libs/kvelt/README.md", """
# kvelt

Pure-Python embedded key-value store. No compiler, no native code: a single `py3-none-any` wheel.

- Python 3.8+
- ACID transactions spanning multiple keys
- `kvelt.aio` for asyncio
- Apache-2.0
""")
write("libs/kvelt/LICENSE", APACHE)
write("libs/kvelt/CHANGELOG.md", changelog("kvelt", [
    ("3.2.0", "2026-09-10", ["Snapshot reads in `kvelt.aio`."]),
    ("3.1.0", "2026-04-22", []),
    ("3.0.0", "2025-12-05", ["**Breaking:** dropped support for Python 3.8 and 3.9; kvelt now requires Python 3.10 or newer (uses `match` statements).", "New on-disk format."]),
    ("2.9.0", "2025-08-14", ["Last release supporting Python 3.8 and 3.9."]),
    ("2.5.0", "2024-11-01", ["Added `kvelt.aio` (asyncio)."]),
    ("2.0.0", "2024-02-18", ["Transactions can span multiple keys (ACID, snapshot isolation)."]),
]))
write("libs/kvelt/docs/install.md", "# Installing kvelt\n\n```\npip install kvelt\n```\n\nkvelt is pure Python and installs on any platform.\n")
write("libs/kvelt/docs/compat.md", "# Compatibility\n\n| kvelt | Python |\n|---|---|\n| 2.x | 3.8 - 3.12 |\n\n*(This table has not been updated since 2.9.)*\n")
write("libs/kvelt/docs/transactions.md", "# Transactions\n\n`with db.tx() as t:` groups reads and writes on any keys into one ACID transaction.\n")
write("libs/kvelt/docs/aio.md", "# asyncio\n\n`kvelt.aio.open()` returns an async database handle.\n")

# ---------------- burrow: satisfies everything ----------------
write("libs/burrow/README.md", """
# burrow

Embedded key-value store for Python built on a small C core.

- Multi-key ACID transactions
- MIT licensed
- No asyncio support yet (use a thread pool)
- Wheels for x86_64 Linux, macOS and Windows

Latest release: 2.2.0 (2025-03-02)
""")
write("libs/burrow/LICENSE", MIT.format(y=2023, who="The burrow authors"))
write("libs/burrow/CHANGELOG.md", changelog("burrow", [
    ("2.5.1", "2026-06-15", ["Fixed a rare fsync ordering bug on macOS."]),
    ("2.5.0", "2026-02-20", ["Dropped Python 3.8. Python 3.9 - 3.13 are supported."]),
    ("2.4.0", "2025-10-01", ["New asyncio API: `burrow.aio` (`await db.get(...)`, async transactions)."]),
    ("2.3.0", "2025-06-12", ["Pre-built manylinux aarch64 wheels."]),
    ("2.2.0", "2025-03-02", []),
    ("2.0.0", "2024-05-09", ["Transactions spanning multiple keys (ACID)."]),
]))
write("libs/burrow/docs/install.md", "# Installing burrow\n\n```\npip install burrow\n```\n\nWheels: see the changelog for the platforms covered by the current release. Building from source needs a C compiler.\n")
write("libs/burrow/docs/faq.md", "# FAQ (2025-03)\n\n**aarch64 Linux?** Not yet; you need a compiler to build from source.\n\n**asyncio?** Planned.\n")
write("libs/burrow/docs/transactions.md", "# Transactions\n\n`with db.begin() as tx:` - any number of keys, all-or-nothing, serializable.\n")
write("libs/burrow/docs/aio.md", "# burrow.aio\n\nAvailable since 2.4.0.\n\n```python\nimport burrow.aio\ndb = await burrow.aio.open('data')\nasync with db.begin() as tx:\n    await tx.put(b'k', b'v')\n```\n")

write("blog/2025-04-kv-shootout.md", """
# Embedded KV stores for Python: a shootout (April 2025)

| | stashdb | burrow | kvelt |
|---|---|---|---|
| License | MIT | MIT | Apache-2.0 |
| Python 3.8 | no | yes | yes |
| asyncio | yes | no | yes |
| aarch64 wheels | no | no | pure Python |

Verdict: kvelt if you need broad Python support, stashdb for speed.
""")
for i in range(1, 6):
    lib = rng.choice(["stashdb", "burrow", "kvelt"])
    write(f"blog/notes-{i:02d}.md", f"# Notes {i}\n\nWe tried {lib} for a cache layer. " + " ".join(rng.choice(NOISE).format(n=rng.randint(5, 40)) for _ in range(5)) + "\n")
write("REQUIREMENTS.md", """
# Requirements for our embedded key-value store

- **C1** The current release is under a permissive open-source license (MIT, BSD or Apache-2.0).
- **C2** The current release supports Python 3.9.
- **C3** It has an asyncio API.
- **C4** It installs on Linux aarch64 without a compiler (pre-built wheel or pure Python).
- **C5** It supports ACID transactions spanning multiple keys.
- **C6** It had a release within the 12 months before 2026-10-01.
""")
