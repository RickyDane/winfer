import { describe, expect, it } from "vitest";
import { followedTab } from "./follow";
import { normalizeUrl } from "./url";

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
  const tab = (id: string, url: string, byAgent = false) => ({ id, url, title: "", byAgent });

  it("ignores the first listing", () => expect(followedTab(undefined, [tab("a", "https://x.dev", true)], null)).toBeUndefined());
  it("follows a tab an agent opened", () => expect(followedTab([tab("a", "about:blank")], [tab("a", "about:blank"), tab("b", "", true)], "a")?.id).toBe("b"));
  it("doesn't follow a tab the UI opened", () => expect(followedTab([tab("a", "about:blank")], [tab("a", "about:blank"), tab("b", "")], "a")).toBeUndefined());
  it("follows a navigation in a background tab", () =>
    expect(followedTab([tab("a", "https://a.dev"), tab("b", "about:blank")], [tab("a", "https://a.dev"), tab("b", "https://b.dev")], "a")?.id).toBe("b"));
  it("ignores title-only changes and the shown tab", () => {
    expect(followedTab([tab("a", "https://a.dev"), tab("b", "https://b.dev")], [tab("a", "https://a.dev"), { ...tab("b", "https://b.dev"), title: "B" }], "a")).toBeUndefined();
    expect(followedTab([tab("a", "https://a.dev")], [tab("a", "https://a.dev/next")], "a")).toBeUndefined();
  });
});
