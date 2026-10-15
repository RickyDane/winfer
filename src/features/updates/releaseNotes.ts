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
