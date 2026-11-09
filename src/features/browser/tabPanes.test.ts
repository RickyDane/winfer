import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const workspace = vi.hoisted(() => ({
  workspaces: [{ id: "ws", panes: [{ id: "a" }, { id: "b" }] }],
  activeWorkspaceId: "ws",
  focusedPaneId: "b",
  view: "workspace",
}));
const events = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@/stores/workspaces", () => ({ useWorkspaces: { getState: () => workspace } }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ pasteText: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => []) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
  events.set(name, handler);
  return () => {};
}) }));

import { ensureBrowser, linkTabPane, markAgentTab, selectTab, tabPane, useBrowser } from "./browser";

const tab = (id: string) => ({ id, url: "https://example.com", title: "Example", favicon: "" });

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("window", globalThis);
  useBrowser.setState({ tabs: { ws: [tab("one"), tab("two")] }, tabPanes: {}, agentTabs: {}, shown: "ws", activeTab: { ws: "one" }, availability: "ready" });
});
afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("pane/tab links", () => {
  it("uses the explicit caller, not the focused pane, in both directions", () => {
    markAgentTab("one", "ws", false, "a");
    linkTabPane("two", "ws", "b");
    const s = useBrowser.getState();
    expect(tabPane(s, "ws", "one")).toBe("a");
    expect(tabPane(s, "ws", "two")).toBe(workspace.focusedPaneId);
    selectTab("two", "ws");
    expect(tabPane(useBrowser.getState(), "ws", useBrowser.getState().activeTab.ws)).toBe("b");
    expect(workspace.focusedPaneId).toBe("b");
  });

  it("retains ownership after activity expires and unattributed CDP activity", () => {
    markAgentTab("one", "ws", false, "a");
    markAgentTab("one", "ws", false);
    vi.advanceTimersByTime(4000);
    expect(useBrowser.getState().agentTabs.one).toBeUndefined();
    expect(tabPane(useBrowser.getState(), "ws", "one")).toBe("a");
  });

  it("transfers a shared tab to its latest explicit caller", () => {
    linkTabPane("one", "ws", "a");
    linkTabPane("one", "ws", "b");
    expect(tabPane(useBrowser.getState(), "ws", "one")).toBe("b");
  });

  it("does not guess ownership or link across workspaces", () => {
    expect(tabPane(useBrowser.getState(), "ws", "one")).toBeNull();
    linkTabPane("one", "other", "a");
    expect(useBrowser.getState().tabPanes).toEqual({});
  });

  it("ignores closed tabs, removed panes and other workspaces", () => {
    linkTabPane("one", "ws", "a");
    const s = useBrowser.getState();
    expect(tabPane(s, "other", "one")).toBeNull();
    expect(tabPane(s, "ws", null)).toBeNull();
    expect(tabPane(s, "ws", "one", [])).toBeNull();
    expect(tabPane({ ...s, tabs: {} }, "ws", "one")).toBeNull();
  });

  it("prunes ownership when a fresh tab listing closes the tab", async () => {
    linkTabPane("one", "ws", "a");
    await ensureBrowser("ws");
    events.get("browser://tabs")!({ payload: { workspaceId: "ws", tabs: [tab("two")] } });
    expect(useBrowser.getState().tabPanes.one).toBeUndefined();
    expect(tabPane(useBrowser.getState(), "ws", "one")).toBeNull();
  });
});
