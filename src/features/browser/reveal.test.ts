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
import { allowInvalidCertificates, ensureBrowser, focusUrlBar, openUrl, useBrowser } from "./browser";
import { useUndock } from "./undockState";

beforeEach(async () => {
  vi.clearAllMocks();
  workspace.browserOpen = false;
  useUndock.setState({ win: null, root: null });
  useBrowser.setState({ tabs: { ws: [] }, activeTab: {}, shown: "ws", availability: "ready", focusUrlBar: 0, invalidCertificates: {} });
  await ensureBrowser("ws");
});

describe("certificate policy", () => {
  it("is secure by default and scopes the opt-in to one workspace", async () => {
    expect(useBrowser.getState().invalidCertificates.ws).toBeUndefined();
    await allowInvalidCertificates("ws", true);
    expect(invoke).toHaveBeenCalledWith("browser_allow_invalid_certificates", { workspaceId: "ws", allow: true });
    expect(useBrowser.getState().invalidCertificates).toEqual({ ws: true });
    await allowInvalidCertificates("other", true);
    await allowInvalidCertificates("ws", false);
    expect(useBrowser.getState().invalidCertificates).toEqual({ ws: false, other: true });
  });

  it("does not change the displayed policy if Chrome rejects the change", async () => {
    vi.stubGlobal("window", { setTimeout: vi.fn(() => 0) });
    vi.mocked(invoke).mockRejectedValueOnce(new Error("disconnected"));
    try {
      await allowInvalidCertificates("ws", true);
      expect(useBrowser.getState().invalidCertificates.ws).toBeUndefined();
      expect(useBrowser.getState().notice).toContain("Couldn't change certificate checks");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("clears the warning when the backend starts a fresh browser", async () => {
    await allowInvalidCertificates("ws", true);
    events.get("browser://certificate-policy")!({ payload: { workspaceId: "ws", allow: false } });
    expect(useBrowser.getState().invalidCertificates.ws).toBe(false);
  });
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
