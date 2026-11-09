import { beforeEach, describe, expect, it, vi } from "vitest";

const workspace = vi.hoisted(() => ({
  workspaces: [{ id: "a" }, { id: "b" }],
  activeWorkspaceId: "a",
  view: "workspace",
}));
const events = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@/stores/workspaces", () => ({ useWorkspaces: { getState: () => workspace } }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ pasteText: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => "blank") }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
  events.set(name, handler);
  return () => {};
}) }));

import { invoke } from "@tauri-apps/api/core";
import { closeTab, ensureBrowser, useBrowser } from "./browser";

const tab = (id: string) => ({ id, url: "https://example.com", title: "Example", favicon: "" });
const update = (workspaceId: string, tabs: ReturnType<typeof tab>[]) => events.get("browser://tabs")!({ payload: { workspaceId, tabs } });

beforeEach(async () => {
  vi.clearAllMocks();
  useBrowser.setState({ tabs: { a: [tab("one")], b: [tab("two")] }, activeTab: { a: "one", b: "two" }, shown: "a", availability: "ready", tabPanes: {} });
  await ensureBrowser("a");
});

describe("closing browser tabs across projects", () => {
  it("closes a project's last tab without replacing it and shows a remaining project", async () => {
    await closeTab("one", "a");
    expect(invoke).toHaveBeenCalledWith("browser_close_tab", { workspaceId: "a", targetId: "one" });
    update("a", []);
    expect(useBrowser.getState().tabs.a).toEqual([]);
    expect(useBrowser.getState().shown).toBe("b");
    expect(useBrowser.getState().activeTab.b).toBe("two");
    expect(invoke).not.toHaveBeenCalledWith("browser_new_tab", expect.anything());
    await ensureBrowser("a");
    expect(invoke).not.toHaveBeenCalledWith("browser_open", expect.anything());
  });

  it("does not switch away when a background project's last tab closes", () => {
    update("b", []);
    expect(useBrowser.getState().shown).toBe("a");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("replaces only the actual final tab with one blank tab", async () => {
    update("a", []);
    update("b", []);
    update("a", []);
    update("b", []);
    await Promise.resolve();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("browser_new_tab", { workspaceId: "b", url: "about:blank" });
    expect(useBrowser.getState().shown).toBe("b");
    expect(useBrowser.getState().activeTab.b).toBe("blank");
  });

  it("does not count tabs belonging to removed projects", async () => {
    useBrowser.setState({ tabs: { a: [tab("one")], removed: [tab("stale")] } });
    update("a", []);
    await Promise.resolve();
    expect(invoke).toHaveBeenCalledWith("browser_new_tab", { workspaceId: "a", url: "about:blank" });
  });
});
