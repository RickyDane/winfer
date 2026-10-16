import { beforeEach, describe, expect, it, vi } from "vitest";

const backend = vi.hoisted(() => ({ plugins: [] as unknown[], source: "" }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "plugins_list") return backend.plugins;
    if (cmd === "plugins_source") return backend.source;
    if (cmd === "providers_list") return [];
    return null;
  }),
}));
vi.mock("@/stores/toasts", () => ({ toast: vi.fn() }));

import { fillTemplate, loadPlugins, runPanelAction, usePlugins, matchesSearch, pluginAgentToolList, pluginPages, runPluginAgentTool, pluginRecipes, pluginToolId, pluginTools, runPluginCommand, setPluginDispatch, toolFromSpec } from "./plugins";
import { toast } from "@/stores/toasts";
import { useWorkspaces } from "@/stores/workspaces";

const plugin = (id: string, manifest: Record<string, unknown>, enabled = true) => ({
  id,
  dir: `/plugins/${id}`,
  enabled,
  fromStore: false,
  error: null,
  manifest: { id, name: id, providers: [], recipes: [], tools: [], ...manifest },
});

describe("declarative tools", () => {
  it("fills {{param}} placeholders in every string", () => {
    expect(fillTemplate({ type: "pane.sendText", text: "run {{ script }} on {{env}}", n: 2, list: ["{{env}}"] }, { script: "test", env: "ci" })).toEqual({
      type: "pane.sendText",
      text: "run test on ci",
      n: 2,
      list: ["ci"],
    });
    expect(fillTemplate("{{missing}}!", {})).toBe("!");
  });

  it("turns a spec into a registry tool with namespaced id, params and palette", () => {
    const tool = toolFromSpec("my.plugin", {
      id: "deploy",
      title: "Deploy",
      description: "Deploy to an environment",
      risk: "confirm",
      params: { env: { kind: "enum", ask: "Which environment?", options: { staging: "Staging", prod: null } } },
      commands: [{ type: "pane.spawn", provider: "shell", prompt: "./deploy {{env}}" }],
      palette: [{ title: "Deploy to staging", args: { env: "staging" } }],
    });
    expect(tool.id).toBe("my_plugin__deploy");
    expect(tool.risk).toBe("confirm");
    const param = tool.params.env;
    expect(param.kind === "enum" && param.options({} as never)).toEqual({ staging: "Staging", prod: null });
    expect(tool.palette!({} as never)).toEqual([{ title: "Deploy to staging", args: { env: "staging" } }]);
    expect(tool.toCommands({ env: "prod" }, {} as never)).toEqual([{ type: "pane.spawn", provider: "shell", prompt: "./deploy prod" }]);
  });

  it("rejects incomplete tools with a readable reason", () => {
    expect(() => toolFromSpec("p", { id: "x", title: "X" })).toThrow(/description/);
    expect(() => toolFromSpec("p", { id: "x", title: "X", description: "d" })).toThrow(/commands/);
    expect(() => toolFromSpec("p", { id: "x", title: "X", description: "d", commands: [], params: { a: { kind: "nope", ask: "?" } } })).toThrow(/unknown kind/);
  });
});

describe("loading plugins", () => {
  beforeEach(() => {
    backend.plugins = [];
    backend.source = "";
    vi.mocked(toast).mockClear();
  });

  it("registers tools and recipes of enabled plugins only", async () => {
    backend.plugins = [
      plugin("tests", {
        tools: [{ id: "run", title: "Run tests", description: "Run the tests", palette: true, commands: [{ type: "ui.toggleSidebar" }] }, { id: "bad" }],
        recipes: [{ id: "pair", name: "Pair", panes: [{ provider: "claude" }], preset: "columns" }],
      }),
      plugin("off", { tools: [{ id: "x", title: "X", description: "d", commands: [] }] }, false),
    ];
    await loadPlugins();
    expect(pluginTools().map((t) => t.id)).toEqual([pluginToolId("tests", "run")]);
    expect(pluginRecipes()).toEqual([{ id: "plugin:tests:pair", name: "Pair", panes: [{ provider: "claude", prompt: undefined }], preset: "columns", url: undefined }]);
  });

  it("runs a code plugin: tools, commands, events and cleanup", async () => {
    const executed: unknown[] = [];
    setPluginDispatch((cmd) => executed.push(cmd));
    backend.source = `
      export function activate(api) {
        api.registerTool({ id: "hi", title: "Say hi", description: "Greets", palette: true, run: (args) => api.execute({ type: "ui.toggleSidebar", from: args.who }) });
        api.registerCommand("ping", () => api.toast("pong"));
        return api.on("pane.status", (e) => api.toast(e.paneId + " " + e.status));
      }`;
    backend.plugins = [plugin("code", { main: "index.js" })];
    await loadPlugins();

    const [tool] = pluginTools();
    expect(tool.id).toBe("code__hi");
    const commands = tool.toCommands({ who: "me" }, {} as never);
    expect(commands).toEqual([{ type: "plugin.run", plugin: "code", command: "tool:hi", args: { who: "me" } }]);
    const run = commands[0] as { plugin: string; command: string; args: Record<string, unknown> };
    await runPluginCommand(run.plugin, run.command, run.args);
    expect(executed).toEqual([{ type: "ui.toggleSidebar", from: "me" }]);
    await runPluginCommand("code", "ping");
    expect(toast).toHaveBeenCalledWith("pong", "info");

    // Disabling the plugin removes what it registered.
    backend.plugins = [plugin("code", { main: "index.js" }, false)];
    await loadPlugins();
    expect(pluginTools()).toEqual([]);
    await runPluginCommand("code", "ping");
    expect(toast).toHaveBeenLastCalledWith(expect.stringContaining("has no command"), "error");
  });

  it("reports a plugin whose code throws without breaking the others", async () => {
    backend.source = "export function activate() { throw new Error('boom'); }";
    backend.plugins = [plugin("crashy", { main: "index.js", tools: [{ id: "ok", title: "OK", description: "d", commands: [] }] })];
    const { usePlugins } = await import("./plugins");
    await loadPlugins();
    expect(usePlugins.getState().runtimeErrors.crashy).toMatch(/boom/);
    expect(pluginTools().map((t) => t.id)).toEqual(["crashy__ok"]);
    expect(useWorkspaces.getState().providers).toEqual([]);
  });

  it("registers pages and tools for agents", async () => {
    backend.source = `
      export function activate(api) {
        api.registerPage({ id: "main", title: "Vault", icon: "key", mount: (el) => { el.textContent = "hi"; } });
        api.registerAgentTool({
          name: "whoami",
          description: "Says who called",
          inputSchema: { type: "object", properties: { x: { type: "number" } } },
          run: (args, caller) => caller.label + " in " + caller.workspaceName + " x=" + args.x,
        });
        api.registerAgentTool({ name: "bare", description: "No schema", run: () => "ok" });
      }`;
    backend.plugins = [plugin("pages", { main: "index.js" })];
    await loadPlugins();

    const [page] = pluginPages();
    expect(page).toMatchObject({ key: "pages/main", title: "Vault", pluginId: "pages" });
    expect(pluginAgentToolList()).toEqual([
      { name: "whoami", description: "Says who called", inputSchema: { type: "object", properties: { x: { type: "number" } } } },
      { name: "bare", description: "No schema", inputSchema: { type: "object", properties: {} } },
    ]);
    const caller = { paneId: "p1", workspaceId: "w1", workspaceName: "home", cwd: "/x", provider: "claude", label: "pane 1 (Claude Code)" };
    expect(await runPluginAgentTool("whoami", { x: 2 }, caller)).toBe("pane 1 (Claude Code) in home x=2");
    expect(await runPluginAgentTool("nope", {}, caller)).toBeNull();

    backend.plugins = [plugin("pages", { main: "index.js" }, false)];
    await loadPlugins();
    expect(pluginPages()).toEqual([]);
    expect(pluginAgentToolList()).toEqual([]);
  });
});

describe("panel actions", () => {
  it("adds, updates, runs and removes buttons on side panels", async () => {
    const executed: unknown[] = [];
    setPluginDispatch((cmd) => executed.push(cmd));
    backend.source = `
      export function activate(api) {
        api.registerPanelAction({ id: "rec", panel: "android", title: "Record", icon: "record", run: () => api.execute({ type: "ui.toggleSidebar" }) });
        api.registerCommand("tick", () => api.updatePanelAction("rec", { label: "0:01", active: true }));
        try { api.registerPanelAction({ id: "bad", panel: "terminal", title: "x", run() {} }); } catch (e) { api.toast(e.message); }
      }`;
    backend.plugins = [plugin("rec", { main: "index.js" })];
    await loadPlugins();

    expect(usePlugins.getState().panelActions).toMatchObject([{ key: "rec/rec", panel: "android", title: "Record", icon: "record" }]);
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("panel must be browser or android"), "info");
    await runPluginCommand("rec", "tick");
    const [action] = usePlugins.getState().panelActions;
    expect(action).toMatchObject({ title: "Record", label: "0:01", active: true });
    await runPanelAction(action);
    expect(executed).toEqual([{ type: "ui.toggleSidebar" }]);

    backend.plugins = [plugin("rec", { main: "index.js" }, false)];
    await loadPlugins();
    expect(usePlugins.getState().panelActions).toEqual([]);
  });
});

describe("store search", () => {
  it("needs every word to match some field", () => {
    expect(matchesSearch("", ["Secrets"])).toBe(true);
    expect(matchesSearch("key secr", ["Secrets", "Keep API keys"])).toBe(true);
    expect(matchesSearch("passwords", ["Secrets", null, "security passwords"])).toBe(true);
    expect(matchesSearch("docker", ["Secrets"])).toBe(false);
  });
});
