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

/** Orders "0.1.9" before "0.1.10"; a leading "v" is ignored. */
export function compareVersions(a: string, b: string): number {
  const parts = (v: string) => v.replace(/^v/, "").split(/[.-]/).map((p) => Number.parseInt(p, 10) || 0);
  const pa = parts(a);
  const pb = parts(b);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d) return d;
  }
  return 0;
}

/**
 * The releases after `from` up to and including `to`, newest first, so skipping versions still
 * shows everything that changed. No `from` → just `to`.
 */
export function releasesBetween(releases: { tag_name: string; body?: string | null; draft?: boolean; prerelease?: boolean }[], from: string | null, to: string): ReleaseChanges[] {
  return releases
    .filter((r) => !r.draft && !r.prerelease)
    .map((r) => ({ version: r.tag_name.replace(/^v/, ""), changes: parseChanges(r.body) }))
    .filter((r) => compareVersions(r.version, to) <= 0 && (from ? compareVersions(r.version, from) > 0 : compareVersions(r.version, to) === 0))
    .sort((a, b) => compareVersions(b.version, a.version));
}
