/** `@path` mentions in the composer: finding the one under the caret and ranking files for it,
 *  the same way the TUI does (`tui::picker::score`, filename bonus, shorter paths first). */

export type Mention = { start: number; end: number; query: string };

/** The `@word` the caret is in (or right after), if any. */
export function mentionAt(text: string, caret: number): Mention | null {
  let start = caret;
  while (start > 0 && !/\s/.test(text[start - 1])) start--;
  if (text[start] !== "@") return null;
  let end = caret;
  while (end < text.length && !/\s/.test(text[end])) end++;
  return { start, end, query: text.slice(start + 1, caret) };
}

/** Fuzzy subsequence score (higher is better), null when `query` doesn't match `hay`. */
export function score(query: string, hay: string): number | null {
  const q = [...query.toLowerCase().replace(/\s/g, "")];
  const h = hay.toLowerCase();
  if (!q.length) return 0;
  let s = 0;
  let qi = 0;
  let last = -2;
  for (let i = 0; i < h.length && qi < q.length; i++) {
    if (h[i] !== q[qi]) continue;
    s += 10;
    if (last === i - 1) s += 15;
    if (i === 0 || "/ -_.".includes(h[i - 1])) s += 20;
    last = i;
    qi++;
  }
  if (qi < q.length) return null;
  if (h.includes(query.toLowerCase())) s += 50;
  return s - Math.floor(h.length / 4);
}

/** The best `limit` files for `query`. */
export function rankFiles(files: string[], query: string, limit = 8): string[] {
  const scored: [number, string][] = [];
  for (const f of files) {
    const s = score(query, f);
    if (s == null) continue;
    const name = f.slice(f.lastIndexOf("/") + 1);
    scored.push([s + (score(query, name) != null ? 30 : 0), f]);
  }
  scored.sort((a, b) => b[0] - a[0] || a[1].length - b[1].length);
  return scored.slice(0, limit).map(([, f]) => f);
}
