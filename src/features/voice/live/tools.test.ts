import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
const storage = new Map<string, string>();
vi.stubGlobal("localStorage", { getItem: (k: string) => storage.get(k) ?? null, setItem: (k: string, v: string) => storage.set(k, v) });
vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(async () => false), sendNotification: vi.fn() }));
vi.mock("@/commands/bus", () => ({ execute: vi.fn() }));

const { registryTool, webTools, runVoiceFunction } = await import("./tools");
const { TOOLS } = await import("@/tools/registry");
const bus = await import("@/commands/bus");

const ctx = {
  providers: [],
  workspaces: [],
  active: { id: "ws", name: "api", rootDir: "~", panes: [], layout: null },
  focusedPaneId: null,
  browserTabs: [],
  lastCommands: [],
  models: {},
  recentAddresses: {},
} as never;

describe("live voice tools", () => {
  it("turns registry params into JSON schema and gates destructive tools", () => {
    const resize = registryTool(TOOLS.find((t) => t.id === "resize_pane")!, ctx);
    expect(resize.type).toBe("function");
    expect(Object.keys(resize.parameters.properties as object)).toEqual(expect.arrayContaining(["target"]));
    const closeAll = registryTool(TOOLS.find((t) => t.id === "close_all_panes")!, ctx);
    expect(closeAll.description).toContain("confirm first");
    expect((closeAll.parameters.properties as Record<string, unknown>).confirmed).toBeDefined();
  });

  it("names Playwright tools web_* and drops hidden ones", () => {
    const tools = webTools([{ name: "browser_click", description: "Click\nmore" }, { name: "browser_drag" }], ["browser_drag"]);
    expect(tools.map((t) => [t.name, t.description])).toEqual([["web_click", "Click"]]);
  });

  it("asks for confirmation before running a destructive tool", async () => {
    expect(await runVoiceFunction("close_all_panes", {}, () => {})).toContain("confirm");
    expect(bus.execute).not.toHaveBeenCalled();
  });

  it("asks to load browser tools before using them", async () => {
    expect(await runVoiceFunction("web_click", {}, () => {})).toBe("Call use_browser first.");
  });
});
