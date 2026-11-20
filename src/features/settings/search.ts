/**
 * Settings search: matches a query against every page and the settings in the search index.
 * The index is searchIndex.generated.json (every label and hint on the pages, made by
 * scripts/gen-settings-index.mjs) plus searchIndex.json (words people use for a setting that
 * its page doesn't, like "yolo"). The native app's settings.rs reads both and ranks the same way.
 */
import GENERATED from "./searchIndex.generated.json";
import HAND from "./searchIndex.json";

export interface SettingsHit {
  page: string;
  label: string;
  words: string;
}

/** Lower-case words (letters and digits) of a text: "Auto-approve (yolo)" → auto, approve, yolo. */
export const wordsOf = (text: string): string[] => text.toLowerCase().match(/[\p{L}\p{N}]+/gu) ?? [];

/** Every query word starts a word of the text:
 *  "host" finds "Host / Port", but "port" does not find "Export". */
export function matches(text: string, query: string[]): boolean {
  const hay = wordsOf(text);
  return query.every((q) => hay.some((w) => w.startsWith(q)));
}

/** The index with labels that appear twice (on the same page) merged into one entry. */
function buildIndex(): SettingsHit[] {
  const merged = new Map<string, SettingsHit>();
  for (const e of [...(HAND as SettingsHit[]), ...(GENERATED as SettingsHit[])]) {
    const key = `${e.page}\0${e.label}`;
    const have = merged.get(key);
    if (have) have.words = `${have.words} ${e.words}`.trim();
    else merged.set(key, { ...e });
  }
  return [...merged.values()];
}
export const INDEX: SettingsHit[] = buildIndex();

/** Settings whose label, words or page match every word of `query`; labels that start with the
 *  first word come first. `pages` are the page ids, labels and descriptions. */
export function searchSettings(query: string, pages: { id: string; label: string; description: string }[]): SettingsHit[] {
  const words = wordsOf(query);
  if (!words.length) return [];
  const pageLabel = (id: string) => pages.find((p) => p.id === id)?.label ?? "";
  const all: SettingsHit[] = [...pages.map((p) => ({ page: p.id, label: p.label, words: p.description })), ...INDEX];
  const hits = all
    .filter((h) => matches(`${h.label} ${h.words} ${pageLabel(h.page)}`, words))
    .map((h) => ({ h, rank: wordsOf(h.label)[0]?.startsWith(words[0]) ? 0 : matches(h.label, words) ? 1 : 2 }))
    .sort((a, b) => a.rank - b.rank)
    .map((x) => x.h);
  const seen = new Set<string>();
  return hits.filter((h) => !seen.has(h.page + "\0" + h.label) && !!seen.add(h.page + "\0" + h.label));
}
