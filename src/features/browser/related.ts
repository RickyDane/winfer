import type { Tab } from "./browser";

/** Loopback hosts are one machine: localhost:5173 and 127.0.0.1:5173 are the same dev server. */
const LOOPBACK = /^(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])$/i;

function site(url: URL) {
  const host = LOOPBACK.test(url.hostname) ? "localhost" : url.hostname.replace(/^www\./, "");
  return `${url.protocol === "https:" && host === "localhost" ? "http:" : url.protocol}//${host}:${url.port}`;
}

function parse(url: string) {
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:" || u.protocol === "file:" ? u : null;
  } catch {
    return null;
  }
}

/** Path segments two URLs share from the start. */
function sharedSegments(a: URL, b: URL) {
  const x = a.pathname.split("/").filter(Boolean);
  const y = b.pathname.split("/").filter(Boolean);
  let n = 0;
  while (n < x.length && n < y.length && x[n] === y[n]) n++;
  return n;
}

/**
 * The open tab that already shows related content for `url` (the same site or dev server), so an
 * agent takes it over instead of opening a duplicate. Among several, the one whose page is
 * closest: the same URL, then the longest shared path, then the most recent.
 */
export function relatedTab(tabs: Tab[], url: string): Tab | undefined {
  const target = parse(url);
  if (!target) return undefined;
  const key = target.protocol === "file:" ? null : site(target);
  let best: { tab: Tab; score: number } | undefined;
  for (const tab of tabs) {
    const u = parse(tab.url);
    if (!u) continue;
    // Local files are related when they're in the same folder.
    const same = key === null ? u.protocol === "file:" && u.pathname.replace(/[^/]*$/, "") === target.pathname.replace(/[^/]*$/, "") : site(u) === key;
    if (!same) continue;
    const exact = u.href.replace(/#.*$/, "") === target.href.replace(/#.*$/, "");
    const score = (exact ? 1000 : 0) + sharedSegments(u, target);
    if (!best || score >= best.score) best = { tab, score };
  }
  return best?.tab;
}
