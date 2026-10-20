/**
 * Release notes as release.yml writes them: a "### Changes" list of commit subjects, then a
 * "---" and the install footer. Only the changes are worth showing in the app.
 */
export function parseChanges(body: string | null | undefined): string[] {
  if (!body) return [];
  const lines = body.split(/\r?\n/);
  const start = lines.findIndex((l) => /^#+\s*changes\s*$/i.test(l.trim()));
  const changes: string[] = [];
  for (const line of lines.slice(start + 1)) {
    const t = line.trim();
    if (t === "---" || (start >= 0 && /^#+\s/.test(t))) break;
    const item = /^[-*]\s+(.+)$/.exec(t);
    if (item) changes.push(item[1]);
  }
  return changes;
}

/** "Plugins: add Telegram" → { area: "Plugins", text: "add Telegram" }; no prefix → no area. */
export function splitArea(change: string): { area: string | null; text: string } {
  const m = /^([A-Z][\w ./+-]{0,30}):\s+(.+)$/.exec(change);
  return m ? { area: m[1], text: m[2] } : { area: null, text: change };
}

/** One release's changes, as the "What's new" dialog lists them. */
export interface ReleaseChanges {
  version: string;
  changes: string[];
}

/**
 * Orders "0.1.9" before "0.1.10"; a leading "v" is ignored. Unstable builds carry a numeric
 * pre-release ("0.1.102-7", see unstable.yml), which sorts before its release, as in semver.
 */
export function compareVersions(a: string, b: string): number {
  const split = (v: string) => {
    const [core, pre] = v.replace(/^v/, "").split(/-(.*)/s);
    const nums = (s: string) => s.split(".").map((p) => Number.parseInt(p, 10) || 0);
    return { core: nums(core), pre: pre ? nums(pre) : null };
  };
  const cmp = (x: number[], y: number[]) => {
    for (let i = 0; i < Math.max(x.length, y.length); i++) {
      const d = (x[i] ?? 0) - (y[i] ?? 0);
      if (d) return d;
    }
    return 0;
  };
  const pa = split(a);
  const pb = split(b);
  const d = cmp(pa.core, pb.core);
  if (d || (!pa.pre && !pb.pre)) return d;
  if (!pa.pre) return 1;
  if (!pb.pre) return -1;
  return cmp(pa.pre, pb.pre);
}

/**
 * The releases after `from` up to and including `to`, newest first, so skipping versions still
 * shows everything that changed. No `from` → just `to`. Unstable (pre-release) builds only count
 * when updating to one; a stable release's notes already cover everything since the last stable.
 */
export function releasesBetween(releases: { tag_name: string; body?: string | null; draft?: boolean; prerelease?: boolean }[], from: string | null, to: string): ReleaseChanges[] {
  const unstable = to.includes("-");
  return releases
    .filter((r) => !r.draft && (unstable || !r.prerelease))
    .map((r) => ({ version: r.tag_name.replace(/^v/, ""), changes: parseChanges(r.body) }))
    .filter((r) => compareVersions(r.version, to) <= 0 && (from ? compareVersions(r.version, from) > 0 : compareVersions(r.version, to) === 0))
    .sort((a, b) => compareVersions(b.version, a.version));
}
