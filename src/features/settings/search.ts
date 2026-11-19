/**
 * Settings search: matches a query against every page and the settings in searchIndex.json
 * (shared with the native app's settings.rs, which ranks the same way).
 */
import INDEX from "./searchIndex.json";

export interface SettingsHit {
  page: string;
  label: string;
  words: string;
}

/** Settings whose label, words or page match every word of `query`; labels that start with the
 *  first word come first. `pages` are the page ids, labels and descriptions. */
export function searchSettings(query: string, pages: { id: string; label: string; description: string }[]): SettingsHit[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const pageLabel = (id: string) => pages.find((p) => p.id === id)?.label ?? "";
  const all: SettingsHit[] = [...pages.map((p) => ({ page: p.id, label: p.label, words: p.description })), ...(INDEX as SettingsHit[])];
  const hits = all
    .filter((h) => {
      const hay = `${h.label} ${h.words} ${pageLabel(h.page)}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    })
    .map((h) => ({ h, rank: h.label.toLowerCase().startsWith(words[0]) ? 0 : 1 }))
    .sort((a, b) => a.rank - b.rank)
    .map((x) => x.h);
  const seen = new Set<string>();
  return hits.filter((h) => !seen.has(h.page + "\0" + h.label) && !!seen.add(h.page + "\0" + h.label));
}
