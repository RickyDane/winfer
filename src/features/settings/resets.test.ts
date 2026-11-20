import { beforeEach, describe, expect, it } from "vitest";
import { useWorkspaces } from "@/stores/workspaces";
import { changedCounts, changedOnPage, RESETS, resetEntry, resetPage } from "./resets";
import INDEX from "./searchIndex.json";

const state = () => useWorkspaces.getState();

describe("settings resets", () => {
  beforeEach(() => {
    resetPage("general");
    resetPage("agents");
    resetPage("usage");
    resetPage("voice");
    resetPage("plugins");
    resetPage("theme");
  });

  it("has unique ids, on pages the search knows", () => {
    expect(new Set(RESETS.map((r) => r.id)).size).toBe(RESETS.length);
    const pages = new Set(INDEX.map((e) => e.page));
    for (const r of RESETS) expect(pages.has(r.page)).toBe(true);
  });

  it("starts with nothing changed", () => {
    expect(changedCounts(state())).toEqual({});
  });

  it("marks a changed setting, and resetting puts it back", () => {
    state().setTerminalGpu(false);
    state().setShowPaneCost(true);
    expect(changedCounts(state())).toEqual({ general: 1, usage: 1 });
    resetEntry("terminal.gpu")!.reset(state());
    expect(state().terminalGpu).toBe(true);
    expect(changedOnPage(state(), "general")).toEqual([]);
  });

  it("resets a whole page and leaves the others alone", () => {
    state().setReviewConfig({ autoTests: true, screenshots: false });
    state().setProgressConfig({ intervalSec: 300 });
    state().setDefaultProvider("codex");
    state().setShowPaneCost(true);
    expect(changedOnPage(state(), "agents")).toHaveLength(4);
    expect(resetPage("agents")).toBe(4);
    expect(state().reviewConfig).toEqual({ autoTests: false, screenshots: true });
    expect(state().defaultProvider).toBeNull();
    expect(state().showPaneCost).toBe(true);
  });

  it("resets the theme, its per-mode picks and following the system", () => {
    state().pickTheme("github-light");
    state().setThemeMode("system");
    expect(changedOnPage(state(), "theme")).toHaveLength(1);
    resetPage("theme");
    expect(state().theme).toBe("dark");
    expect(state().themeMode).toBe("manual");
    expect(state().themePicks).toEqual({});
    expect(changedOnPage(state(), "theme")).toEqual([]);
  });

  it("does not count the default interval as changed", () => {
    state().setProgressConfig({ intervalSec: 60 });
    expect(resetEntry("progress.interval")!.changed(state())).toBe(false);
  });
});
