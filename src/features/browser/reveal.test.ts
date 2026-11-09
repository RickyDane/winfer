import { beforeEach, describe, expect, it, vi } from "vitest";

const workspace = vi.hoisted(() => ({
  workspaces: [{ id: "ws" }],
  activeWorkspaceId: "ws",
  view: "workspace",
  browserOpen: false,
  sidePanel: "viewer",
  toggleBrowser: vi.fn(),
}));
const events = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@/stores/workspaces", () => ({ useWorkspaces: { getState: () => workspace } }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ pasteText: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => "new-tab") }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
  events.set(name, handler);
  return () => {};
}) }));

import { invoke } from "@tauri-apps/api/core";
import { ensureBrowser, focusUrlBar, openUrl, useBrowser } from "./browser";
import { useUndock } from "./undockState";

beforeEach(async () => {
  vi.clearAllMocks();
  workspace.browserOpen = false;
  useUndock.setState({ win: null, root: null });
  useBrowser.setState({ tabs: { ws: [] }, activeTab: {}, shown: "ws", availability: "ready", focusUrlBar: 0 });
  await ensureBrowser("ws");
});

describe.each([false, true])("browser reveal (undocked: %s)", (undocked) => {
  beforeEach(() => {
    if (undocked) useUndock.setState({ win: {} as Window });
  });

  it("opens URLs without opening the side section when undocked", async () => {
    await openUrl("https://example.com");
    expect(invoke).toHaveBeenCalledWith("browser_new_tab", { workspaceId: "ws", url: "https://example.com" });
    expect(useBrowser.getState().activeTab.ws).toBe("new-tab");
    expect(workspace.toggleBrowser).toHaveBeenCalledTimes(undocked ? 0 : 1);
    expect(workspace.sidePanel).toBe("viewer");
  });

  it("selects agent-navigated tabs without opening the side section when undocked", () => {
    events.get("browser://agent-navigate")!({ payload: { workspaceId: "ws", targetId: "agent-tab" } });
    expect(useBrowser.getState().activeTab.ws).toBe("agent-tab");
    expect(workspace.toggleBrowser).toHaveBeenCalledTimes(undocked ? 0 : 1);
  });

  it("still requests address-bar focus", () => {
    focusUrlBar();
    expect(useBrowser.getState().focusUrlBar).toBe(1);
    expect(workspace.toggleBrowser).toHaveBeenCalledTimes(undocked ? 0 : 1);
  });

  it("leaves an already open side section alone", async () => {
    workspace.browserOpen = true;
    await openUrl("https://example.com");
    focusUrlBar();
    events.get("browser://agent-navigate")!({ payload: { workspaceId: "ws", targetId: "agent-tab" } });
    expect(workspace.toggleBrowser).not.toHaveBeenCalled();
    expect(workspace.sidePanel).toBe("viewer");
  });
});
