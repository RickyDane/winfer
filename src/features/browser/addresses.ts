/**
 * Finds web addresses agents mentioned in their output ("open /Users/me/site/index.html",
 * "Local: http://localhost:5173/") so "open the page you made" can open the right one.
 */
export interface FoundAddress {
  url: string;
  /** Where it came from, shown to the voice planner. */
  label: string;
}

const URL_RE = /https?:\/\/[^\s"'<>`)\]]+/g;
const PATH_RE = /(?:^|[\s"'`(=:])((?:~\/|\/)?(?:[\w.@-]+\/)*[\w.@-]+\.html?)\b/g;

function toFileUrl(path: string, cwd: string, home: string): string | null {
  const expand = (p: string) => (p === "~" ? home : p.startsWith("~/") ? `${home}/${p.slice(2)}` : p);
  let abs = expand(path);
  if (!abs.startsWith("/")) {
    const base = expand(cwd);
    if (!base.startsWith("/")) return null;
    abs = `${base.replace(/\/$/, "")}/${abs.replace(/^\.\//, "")}`;
  }
  return `file://${encodeURI(abs)}`;
}

/** Addresses in `text`, newest (last) first, resolved against the pane's folder. */
export function findAddresses(text: string, cwd: string, home: string): string[] {
  const found: { url: string; at: number }[] = [];
  for (const m of text.matchAll(URL_RE)) found.push({ url: m[0].replace(/[.,;:]+$/, "").replace("0.0.0.0", "localhost"), at: m.index! });
  for (const m of text.matchAll(PATH_RE)) {
    // Skip the path part of URLs we already have.
    if (/https?:\/\/\S*$/.test(text.slice(Math.max(0, m.index! - 200), m.index! + 1))) continue;
    const url = toFileUrl(m[1], cwd, home);
    if (url) found.push({ url, at: m.index! + m[0].indexOf(m[1]) });
  }
  const seen = new Set<string>();
  return found
    .sort((a, b) => b.at - a.at)
    .map((f) => f.url)
    .filter((u) => !seen.has(u) && seen.add(u));
}
