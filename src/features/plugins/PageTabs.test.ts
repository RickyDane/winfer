import { describe, expect, it } from "vitest";
import { pinnedPages } from "./PageTabs";
import type { PluginPage } from "./plugins";

const page = (key: string): PluginPage => ({ key, id: key, pluginId: key.split("/")[0], title: key, mount: () => {} });
const pages = ["a/x", "b/x", "c/x", "d/x"].map(page);

describe("pinnedPages", () => {
  it("pins the first few pages until the user chooses", () => {
    expect(pinnedPages(pages, null).map((p) => p.key)).toEqual(["a/x", "b/x", "c/x"]);
  });

  it("keeps the user's order and skips pages that aren't registered", () => {
    expect(pinnedPages(pages, ["d/x", "gone/x", "a/x"]).map((p) => p.key)).toEqual(["d/x", "a/x"]);
  });

  it("shows no tabs when everything is unpinned", () => {
    expect(pinnedPages(pages, [])).toEqual([]);
  });
});
