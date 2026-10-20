import { describe, expect, it } from "vitest";
import { followedTab } from "./follow";
import { normalizeUrl } from "./url";
import { relatedTab } from "./related";

describe("normalizeUrl", () => {
  it.each([
    ["localhost:3000", "http://localhost:3000"],
    ["127.0.0.1:5173/app", "http://127.0.0.1:5173/app"],
    ["github.com", "https://github.com"],
    ["docs.rs/tokio", "https://docs.rs/tokio"],
    ["https://example.com/x", "https://example.com/x"],
    ["about:blank", "about:blank"],
    ["tauri multiwebview", "https://www.google.com/search?q=tauri%20multiwebview"],
  ])("%s → %s", (input, expected) => expect(normalizeUrl(input)).toBe(expected));
});

describe("followedTab", () => {
  const tab = (id: string, url: string) => ({ id, url, title: "" });

  it("ignores the first listing", () => expect(followedTab(undefined, [tab("a", "https://x.dev")], null)).toBeUndefined());
  it("follows a new tab with a page", () => expect(followedTab([tab("a", "about:blank")], [tab("a", "about:blank"), tab("b", "https://b.dev")], "a")?.id).toBe("b"));
  it("doesn't follow a new blank tab", () => expect(followedTab([tab("a", "about:blank")], [tab("a", "about:blank"), tab("b", "")], "a")).toBeUndefined());
  it("follows a navigation in a background tab", () =>
    expect(followedTab([tab("a", "https://a.dev"), tab("b", "about:blank")], [tab("a", "https://a.dev"), tab("b", "https://b.dev")], "a")?.id).toBe("b"));
  it("ignores title-only changes and the shown tab", () => {
    expect(followedTab([tab("a", "https://a.dev"), tab("b", "https://b.dev")], [tab("a", "https://a.dev"), { ...tab("b", "https://b.dev"), title: "B" }], "a")).toBeUndefined();
    expect(followedTab([tab("a", "https://a.dev")], [tab("a", "https://a.dev/next")], "a")).toBeUndefined();
  });
});

describe("relatedTab", () => {
  const tab = (id: string, url: string) => ({ id, url, title: "" });
  const tabs = [tab("gh", "https://github.com/winfer/winfer/pull/42"), tab("dev", "http://localhost:5173/settings"), tab("docs", "https://docs.rs/tokio"), tab("blank", "about:blank")];

  it("finds the tab on the same site", () => expect(relatedTab(tabs, "https://github.com/winfer/winfer/issues")?.id).toBe("gh"));
  it("treats loopback hosts as one dev server", () => expect(relatedTab(tabs, "http://127.0.0.1:5173/")?.id).toBe("dev"));
  it("ignores other ports, sites and blank tabs", () => {
    expect(relatedTab(tabs, "http://localhost:3000/settings")).toBeUndefined();
    expect(relatedTab(tabs, "https://gitlab.com/winfer")).toBeUndefined();
    expect(relatedTab(tabs, "about:blank")).toBeUndefined();
  });
  it("prefers the closest page", () => {
    const two = [tab("a", "https://github.com/other/repo"), tab("b", "https://github.com/winfer/winfer"), tab("c", "https://github.com/")];
    expect(relatedTab(two, "https://github.com/winfer/winfer/pulls")?.id).toBe("b");
    expect(relatedTab(two, "https://www.github.com/other/repo")?.id).toBe("a");
  });
  it("relates local files in the same folder", () =>
    expect(relatedTab([tab("f", "file:///Users/r/site/index.html")], "file:///Users/r/site/about.html")?.id).toBe("f"));
});
