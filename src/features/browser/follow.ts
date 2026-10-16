import type { Tab } from "./browser";

const isBlank = (url: string) => url === "" || url === "about:blank";

/**
 * The tab an agent just opened or navigated, so the panel can show what it's doing. Agents
 * (Playwright) drive their own "current" tab, which needn't be the one on screen.
 */
export function followedTab(previous: Tab[] | undefined, tabs: Tab[], current: string | null | undefined): Tab | undefined {
  if (!previous) return undefined; // first listing: nothing happened yet
  return [...tabs].reverse().find((t) => {
    if (t.id === current) return false;
    const before = previous.find((p) => p.id === t.id);
    // A new tab is worth showing once it has a page; blank ones are followed when they navigate.
    return !isBlank(t.url) && before?.url !== t.url;
  });
}
