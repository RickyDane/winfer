"""Generates the tidepool docs corpus for this task into argv[1] (deterministic)."""

import os
import random
import sys

root = sys.argv[1]
rng = random.Random(101)


def write(path, text):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text.lstrip("\n"))


VERSIONS = (
    ["1.0.0", "1.1.0", "1.2.0", "1.2.1", "1.3.0", "1.4.0", "1.4.1", "1.5.0", "1.6.0"]
    + ["2.0.0", "2.1.0", "2.1.1", "2.2.0", "2.3.0", "2.4.0", "2.4.1", "2.5.0"]
    + ["3.0.0", "3.0.1", "3.1.0", "3.2.0", "3.3.0", "3.4.0", "3.5.0", "3.5.1", "3.5.2", "3.6.0", "3.6.1"]
    + ["4.0.0", "4.0.1", "4.1.0", "4.2.0", "4.2.1"]
)

# Real facts about the artifact cache expiry option (the only ones the task asks about).
FACTS = {
    "1.2.0": ["Added `index.max_age`: how often (in seconds) the upstream package index is re-fetched. Default 600."],
    "1.4.0": ["Added `cache.max_age`: how long a fetched artifact stays in the local cache before it is re-validated, in seconds. Default 3600."],
    "2.3.0": ["Renamed `cache.max_age` to `cache.ttl`. The old name keeps working as a deprecated alias and logs a warning at startup."],
    "3.0.0": ["Removed the deprecated `cache.max_age` alias; configs that still use it now fail to load. Use `cache.ttl`."],
    "3.1.0": ["The default of `cache.ttl` changed from 3600 to 86400 (one day)."],
    "3.5.0": ["Deprecated `cache.ttl` in favour of the new `cache.expire_after` (same meaning, same default)."],
    "3.5.2": ["Reverted the deprecation of `cache.ttl` from 3.5.0 after user feedback: `cache.ttl` is the supported name again and `cache.expire_after` has been removed. Configs that adopted `cache.expire_after` must switch back."],
    "4.0.0": ["The default of `cache.ttl` was lowered from 86400 to 43200 (12 hours) to pick up yanked releases sooner."],
    "4.1.0": ["Renamed `index.max_age` to `index.refresh_interval` (old name still accepted)."],
}

NOISE_OPTS = [
    "cache.max_size", "cache.dir", "cache.compression", "cache.shards", "server.bind", "server.port",
    "server.workers", "server.tls.cert", "server.tls.key", "upstream.url", "upstream.timeout",
    "upstream.retries", "upstream.proxy", "auth.tokens_file", "auth.mode", "log.level", "log.format",
    "log.file", "storage.backend", "storage.s3.bucket", "storage.s3.region", "metrics.enabled",
    "metrics.port", "gc.interval", "gc.min_free", "index.mirror", "prefetch.enabled", "prefetch.depth",
]
TEMPLATES = [
    "Added `{o}` option.", "Fixed a crash when `{o}` is set to an empty string.",
    "`{o}` now accepts environment variable references like `${{VAR}}`.",
    "Improved error message when `{o}` is invalid.", "Fixed a race in the {w} worker.",
    "Faster startup with large caches ({n}% fewer stat calls).", "Updated bundled CA certificates.",
    "The {w} endpoint now returns 404 instead of 500 for unknown packages.",
    "Documented `{o}` in the configuration reference.", "Fixed memory leak in {w} handling.",
    "`tidepool doctor` now checks `{o}`.", "Dropped support for Python {py}.",
    "Logs now include the request id for {w} requests.", "Fixed `{o}` being ignored when set via CLI flag.",
]
WORDS = ["download", "upload", "gc", "metrics", "index", "prefetch", "auth", "mirror", "health"]
DATES = {}
y, m, d = 2019, 2, 11
for v in VERSIONS:
    DATES[v] = f"{y}-{m:02d}-{d:02d}"
    m += rng.choice([1, 2, 3])
    d = rng.randint(1, 28)
    if m > 12:
        y, m = y + 1, m - 12


def noise_line():
    return rng.choice(TEMPLATES).format(o=rng.choice(NOISE_OPTS), w=rng.choice(WORDS), n=rng.randint(10, 60), py=rng.choice(["3.6", "3.7", "3.8"]))


cl = ["# Changelog\n", "All notable changes to tidepool. This file is the authoritative record of what shipped in each release.\n"]
cl.append("## Unreleased\n\n### Changed\n- Planned: lower the default of `cache.ttl` from 43200 to 21600.\n- `server.workers` defaults to the number of CPUs.\n")
for v in reversed(VERSIONS):
    cl.append(f"## {v} ({DATES[v]})\n")
    items = [noise_line() for _ in range(rng.randint(3, 8))]
    for f in FACTS.get(v, []):
        items.insert(rng.randint(0, len(items)), f)
    cl.append("### Changes\n" + "".join(f"- {i}\n" for i in items))
write("CHANGELOG.md", "\n".join(cl))

write("README.md", """
# tidepool

A caching proxy for package registries. Point your installers at tidepool and it keeps
artifacts in a local cache.

- Configuration reference: `docs/<major version>/configuration.md`
- Migration guides: `docs/migration/`
- What changed when: `CHANGELOG.md`
""")


def opt_table(rows):
    out = "| Option | Default | Description |\n|---|---|---|\n"
    for r in rows:
        out += f"| `{r[0]}` | {r[1]} | {r[2]} |\n"
    return out


def noise_rows(k):
    rows = []
    for o in rng.sample(NOISE_OPTS, k):
        rows.append((o, rng.choice(["`true`", "`false`", str(rng.randint(1, 500)), '`""`', "`info`"]), f"Controls {o.split('.')[-1].replace('_', ' ')}."))
    return rows


CONF = {
    "v1": ("1.6", [("cache.max_age", "3600", "Seconds a fetched artifact stays cached before re-validation."), ("index.max_age", "600", "Seconds between index refreshes.")]),
    "v2": ("2.5", [("cache.ttl", "3600", "Seconds a fetched artifact stays cached before re-validation. Formerly `cache.max_age`."), ("index.max_age", "600", "Seconds between index refreshes.")]),
    "v3": ("3.5", [("cache.expire_after", "86400", "Seconds a fetched artifact stays cached before re-validation. Replaces the deprecated `cache.ttl`."), ("index.max_age", "600", "Seconds between index refreshes.")]),
    "v4": ("4.x", [("cache.ttl", "86400", "Seconds a fetched artifact stays cached before re-validation. Introduced in 2.0."), ("index.refresh_interval", "600", "Seconds between index refreshes.")]),
}
for major, (label, rows) in CONF.items():
    allrows = noise_rows(10) + rows
    rng.shuffle(allrows)
    write(f"docs/{major}/configuration.md", f"# Configuration reference (tidepool {label})\n\nAll options go in `tidepool.toml`.\n\n" + opt_table(allrows))
    for page in ["cli", "server", "storage", "auth", "logging", "faq", "gc"]:
        body = f"# {page.upper() if page == 'cli' else page.capitalize()} (tidepool {label})\n\n"
        for _ in range(rng.randint(3, 6)):
            body += f"## {rng.choice(WORDS).capitalize()} {rng.choice(['basics', 'tuning', 'troubleshooting', 'examples'])}\n\n"
            body += " ".join(noise_line() for _ in range(rng.randint(2, 5))) + "\n\n"
        if page == "faq":
            body += "## How long are artifacts kept?\n\nSee the configuration reference for the cache expiry option and `gc.interval` for when expired artifacts are deleted.\n"
        write(f"docs/{major}/{page}.md", body)

write("docs/migration/1-to-2.md", """
# Migrating from 1.x to 2.x

- `server.port` no longer defaults to 80; it defaults to 8080.
- The `auth.mode = "none"` value was renamed to `"open"`.
- Storage backends are now configured under `storage.*` instead of `backend.*`.
""")
write("docs/migration/2-to-3.md", """
# Migrating from 2.x to 3.x

- `cache.max_age` was renamed to `cache.ttl` in 3.0. Update your config.
- Python 3.7 is no longer supported.
- `tidepool serve --daemon` was removed; use your service manager.
""")
write("docs/migration/3-to-4.md", """
# Migrating from 3.x to 4.x

- `cache.expire_after` is the name of the cache expiry option going forward; `cache.ttl` is deprecated.
- `index.max_age` is now `index.refresh_interval`.
- Metrics moved to `/metrics` on the main port unless `metrics.port` is set.
""")
for i in range(1, 9):
    write(f"docs/blog/post-{i:02d}.md", f"# Release notes digest #{i}\n\n" + "\n".join(f"- {noise_line()}" for _ in range(8)) + "\n")
write("docs/blog/post-05.md", "# Tuning tidepool for CI\n\nWe recommend setting `cache.max_age` to a few hours in CI so stale artifacts get re-validated.\n\n" + "\n".join(f"- {noise_line()}" for _ in range(6)) + "\n")
