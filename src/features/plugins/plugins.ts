/**
 * Plugins: folders in the app data dir with a `plugin.json` (see docs/plugins.md and
 * src-tauri/src/plugins.rs). A plugin contributes any of:
 *
 * - `providers`: agent CLIs (handled in Rust; they show up in `providers_list`).
 * - `recipes`: agent setups, listed beside the user's own.
 * - `tools`: declarative actions for the palette and voice that run Command Bus commands.
 * - `main`: an ES module whose `activate(api)` registers tools, command handlers, pages (title
 *   bar tabs), buttons on the browser and Android panels, tools for agents (served by winfer's
 *   MCP server) and keeps secrets in a vault.
 *
 * Plugins come from the store (the repo's plugins/ folder; installing turns them on) or are
 * copied into the plugins folder by hand (those start off until the user turns them on).
 */
import { invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { create } from "zustand";
import type { Command } from "@/commands/bus";
import type { Args, ParamDef, ToolDef } from "@/tools/registry";
import type { Recipe } from "@/features/recipes/recipes";
import { PRESETS } from "@/features/layout/tree";
import { toast } from "@/stores/toasts";
import { providerName, useWorkspaces, type AgentStatus, type Pane, type Workspace } from "@/stores/workspaces";
import { leaves } from "@/features/layout/tree";
import { describeRequest, type AgentRequest } from "@/features/status/request";
import { lastReply } from "@/features/status/replies";

/** Mirrors `Manifest` in src-tauri/src/plugins.rs. */
export interface PluginManifest {
  id: string;
  name: string;
  version?: string | null;
  description?: string | null;
  author?: string | null;
  main?: string | null;
  providers: { id: string; name: string }[];
  recipes: unknown[];
  tools: unknown[];
}

/** Mirrors `PluginInfo` in src-tauri/src/plugins.rs. */
export interface PluginInfo {
  id: string;
  dir: string;
  manifest: PluginManifest | null;
  enabled: boolean;
  /** Installed from the plugin store. */
  fromStore: boolean;
  /** Catalog version installed from the store; may differ from `manifest.version`. */
  storeVersion?: string | null;
  error: string | null;
}

/** Declarative parameters: like `ParamDef`, with fixed options instead of functions. */
export type ParamSpec =
  | { kind: "enum"; ask: string; options: Record<string, string | null> }
  | { kind: "count"; ask: string; max: number }
  | { kind: "bool"; ask: string }
  | { kind: "level"; ask: string; levels: string[] }
  | { kind: "span"; slot: "instruction" | "address"; ask: string };

/** A tool as written in plugin.json or passed to `api.registerTool`. */
export interface ToolSpec {
  id: string;
  title: string;
  description: string;
  risk?: "safe" | "confirm";
  params?: Record<string, ParamSpec>;
  /** Commands to run; string values may contain `{{param}}` placeholders. */
  commands?: Command[];
  /** Runs instead of `commands` (code plugins only). */
  run?: (args: Args) => void | Promise<void>;
  /** Palette entries: true for one entry named after the tool, or a list with fixed arguments. */
  palette?: boolean | { title: string; args?: Args }[];
}

export type PluginEvent =
  | { type: "pane.status"; paneId: string; workspaceId: string; provider: string; status: AgentStatus; previous: AgentStatus }
  | { type: "pane.opened"; paneId: string; workspaceId: string; provider: string }
  | { type: "pane.closed"; paneId: string; workspaceId: string; provider: string };

type Disposer = () => void;

/** A page shown as a title bar tab; `mount` renders into `el` and may return a cleanup. */
export interface PageSpec {
  id: string;
  title: string;
  /** One of PAGE_ICONS' names; defaults to a puzzle piece. */
  icon?: string;
  mount(el: HTMLElement): void | Disposer;
}

export interface PluginPage extends PageSpec {
  /** `<plugin id>/<page id>`; the view is `page:<key>`. */
  key: string;
  pluginId: string;
}

/** Side panels a plugin can add toolbar buttons to. */
export const ACTION_PANELS = ["browser", "android"] as const;
export type ActionPanel = (typeof ACTION_PANELS)[number];

/** How a panel button looks; `updatePanelAction` changes any of it. */
export interface PanelActionLook {
  /** Tooltip. */
  title: string;
  /** One of PAGE_ICONS' names; defaults to a puzzle piece. */
  icon?: string;
  /** Short text beside the icon, e.g. a running time. */
  label?: string;
  /** Drawn highlighted (e.g. while recording). */
  active?: boolean;
}

/** A button on a side panel's toolbar: the browser's address bar or the Android mirror's. */
export interface PanelActionSpec extends PanelActionLook {
  id: string;
  panel: ActionPanel;
  run(): void | Promise<void>;
}

export interface PanelAction extends PanelActionSpec {
  /** `<plugin id>/<action id>`. */
  key: string;
  pluginId: string;
  /** Tells registrations of the same key apart, so undoing an old one leaves a newer one. */
  serial: number;
}

/** The agent calling a plugin's agent tool. */
export interface AgentCaller {
  paneId: string;
  workspaceId: string;
  workspaceName: string;
  /** The pane's working directory (its worktree, if it has one). */
  cwd: string;
  provider: string;
  /** e.g. "pane 2 (Claude Code)". */
  label: string;
}

/** A tool agents can call through winfer's MCP server. */
export interface AgentToolSpec {
  /** Letters, digits, _ and -; must not clash with winfer's own tools. */
  name: string;
  description: string;
  /** JSON Schema of the arguments. */
  inputSchema?: Record<string, unknown>;
  /** Returns the text the agent sees; throw to report an error. */
  run(args: Record<string, unknown>, caller: AgentCaller): string | Promise<string>;
}

/** Mirrors `SecretInfo` in src-tauri/src/vault.rs. */
export interface SecretInfo {
  key: string;
  meta: Record<string, unknown>;
  updatedAt: number;
}

/** A plugin's secrets in the OS keychain. Values never come back except through `reveal`. */
export interface PluginVault {
  list(): Promise<SecretInfo[]>;
  /** Saves metadata, and the value unless it's undefined (then the stored one stays). */
  set(key: string, value: string | undefined, meta?: Record<string, unknown>): Promise<SecretInfo>;
  delete(key: string): Promise<void>;
  /** The value, to show the user; never hand it to an agent. */
  reveal(key: string): Promise<string>;
  /** Runs a shell command with the secrets as environment variables; their values are redacted from the output. */
  exec(opts: { command: string; cwd: string; keys: string[]; timeoutSec?: number }): Promise<{ code: number | null; timedOut: boolean; output: string }>;
  /** Types a secret into a field of the workspace's browser tab (the active one, or the first whose URL contains `tab`). */
  fill(opts: { workspaceId: string; key: string; selector: string; tab?: string }): Promise<string>;
}

/** What a plugin's `activate` receives. */
export interface PluginApi {
  readonly id: string;
  /** The plugin's folder (absolute), e.g. for files shipped with it. */
  readonly dir: string;
  /** The winfer executable, e.g. to run `winfer code`. */
  appExe(): Promise<string>;
  registerTool(spec: ToolSpec): Disposer;
  registerPage(spec: PageSpec): Disposer;
  registerAgentTool(spec: AgentToolSpec): Disposer;
  registerPanelAction(spec: PanelActionSpec): Disposer;
  /** Changes how one of this plugin's panel buttons looks. */
  updatePanelAction(id: string, look: Partial<PanelActionLook>): void;
  /** Shows one of this plugin's pages. */
  openPage(id: string): void;
  vault: PluginVault;
  /** Asks the user; true if they agreed. */
  confirm(message: string, opts?: { title?: string; okLabel?: string; kind?: "info" | "warning" }): Promise<boolean>;
  /** Handles `{ type: "plugin.run", plugin: <this id>, command: name, args }`. */
  registerCommand(name: string, handler: (args: Args) => void | Promise<void>): Disposer;
  /** Runs any Command Bus command. */
  execute(command: Command): void;
  on<T extends PluginEvent["type"]>(type: T, handler: (event: Extract<PluginEvent, { type: T }>) => void): Disposer;
  /** A read-only snapshot of workspaces and panes. */
  state(): { workspaces: Workspace[]; activeWorkspaceId: string; focusedPaneId: string | null };
  /** Names a pane like agent tools' `caller` (workspace, label, provider); null if it's gone. */
  describePane(paneId: string): AgentCaller | null;
  /** The last `lines` lines (default 40) of a pane's terminal as plain text; null if it's gone. */
  readPane(paneId: string, lines?: number): Promise<string | null>;
  /** What an agent waiting for input asks, read from its screen; null if the pane is gone. */
  paneRequest(paneId: string): Promise<AgentRequest | null>;
  /** A file's text (UTF-8), by absolute path or one starting with "~"; reads big files much faster than `vault.exec`. */
  readFile(path: string): Promise<string>;
  /** Writes text or bytes to a file (absolute or "~/…"), creating its folder; resolves to the absolute path. */
  writeFile(path: string, data: string | Uint8Array | ArrayBuffer | Blob): Promise<string>;
  /** Shows a file in Finder / Explorer. */
  revealFile(path: string): Promise<void>;
  /** The agent's final reply from its last finished turn, when its hooks report it; else null. */
  lastReply(paneId: string): string | null;
  toast(text: string, kind?: "info" | "success" | "error"): void;
}

interface PluginModule {
  activate?: (api: PluginApi) => void | Disposer | Promise<void | Disposer>;
  deactivate?: () => void | Promise<void>;
}

interface Runtime {
  tools: ToolDef[];
  pages: PluginPage[];
  agentTools: AgentToolSpec[];
  commands: Map<string, (args: Args) => void | Promise<void>>;
  listeners: Set<(e: PluginEvent) => void>;
  disposers: Disposer[];
  module: PluginModule | null;
}

interface PluginsState {
  plugins: PluginInfo[];
  /** Errors raised while activating or running a plugin's code, by plugin id. */
  runtimeErrors: Record<string, string>;
  /** Bumped whenever contributed tools change, so views re-read them. */
  version: number;
  /** Buttons plugins put on side panels; kept apart from `version` since they change often (labels tick). */
  panelActions: PanelAction[];
}

export const usePlugins = create<PluginsState>(() => ({ plugins: [], runtimeErrors: {}, version: 0, panelActions: [] }));

const runtimes = new Map<string, Runtime>();
let panelActionSerial = 0;
const bump = () => usePlugins.setState((s) => ({ version: s.version + 1 }));

const setRuntimeError = (id: string, error: string | null) =>
  usePlugins.setState((s) => {
    const runtimeErrors = { ...s.runtimeErrors };
    if (error) runtimeErrors[id] = error;
    else delete runtimeErrors[id];
    return { runtimeErrors };
  });

/** Tool ids from plugin and tool id, kept to [A-Za-z0-9_-] so the planner can echo them back. */
export const pluginToolId = (pluginId: string, toolId: string) => `${pluginId}__${toolId}`.replace(/[^A-Za-z0-9_-]/g, "_");

/** Replaces `{{name}}` in every string of `value` with the argument (empty if missing). */
export function fillTemplate<T>(value: T, args: Args): T {
  if (typeof value === "string") {
    return value.replace(/\{\{\s*([\w.-]+)\s*\}\}/g, (_, name: string) => {
      const arg = args[name];
      return arg === undefined || arg === null ? "" : typeof arg === "object" ? JSON.stringify(arg) : String(arg);
    }) as T;
  }
  if (Array.isArray(value)) return value.map((v) => fillTemplate(v, args)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, fillTemplate(v, args)])) as T;
  }
  return value;
}

function toParamDef(spec: ParamSpec): ParamDef {
  if (spec.kind === "enum") {
    const options = { ...spec.options };
    return { kind: "enum", ask: spec.ask, options: () => options };
  }
  return spec;
}

const isObject = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);

function checkParam(name: string, p: unknown): ParamSpec {
  if (!isObject(p) || typeof p.ask !== "string") throw new Error(`param “${name}” needs a kind and an ask`);
  switch (p.kind) {
    case "enum":
      if (!isObject(p.options)) throw new Error(`enum param “${name}” needs options`);
      break;
    case "count":
      if (typeof p.max !== "number") throw new Error(`count param “${name}” needs max`);
      break;
    case "level":
      if (!Array.isArray(p.levels)) throw new Error(`level param “${name}” needs levels`);
      break;
    case "span":
      if (p.slot !== "instruction" && p.slot !== "address") throw new Error(`span param “${name}” needs slot instruction or address`);
      break;
    case "bool":
      break;
    default:
      throw new Error(`param “${name}” has unknown kind “${String(p.kind)}”`);
  }
  return p as unknown as ParamSpec;
}

/** Turns a plugin's tool spec into a registry tool; throws with a readable message if invalid. */
export function toolFromSpec(pluginId: string, raw: unknown): ToolDef {
  if (!isObject(raw)) throw new Error("a tool must be an object");
  const spec = raw as unknown as ToolSpec;
  for (const key of ["id", "title", "description"] as const) {
    if (typeof spec[key] !== "string" || !spec[key]) throw new Error(`tool ${spec.id ?? "?"} needs a ${key}`);
  }
  if (!spec.run && !Array.isArray(spec.commands)) throw new Error(`tool ${spec.id} needs commands`);
  const params = Object.fromEntries(Object.entries(spec.params ?? {}).map(([name, p]) => [name, toParamDef(checkParam(name, p))]));
  const id = pluginToolId(pluginId, spec.id);
  const entries = spec.palette === true ? [{ title: spec.title, args: {} }] : Array.isArray(spec.palette) ? spec.palette : [];
  return {
    id,
    title: spec.title,
    description: spec.description,
    risk: spec.risk === "confirm" ? "confirm" : "safe",
    params,
    toCommands: (args) =>
      // Code tools run through the bus too, so voice and the palette treat them alike.
      spec.run ? [{ type: "plugin.run", plugin: pluginId, command: `tool:${spec.id}`, args }] : (spec.commands ?? []).map((c) => fillTemplate(c, args)),
    palette: entries.length ? () => entries.map((e) => ({ title: e.title, args: e.args ?? {} })) : undefined,
  };
}

function recipeFromSpec(pluginId: string, raw: unknown): Recipe {
  if (!isObject(raw) || typeof raw.id !== "string" || typeof raw.name !== "string" || !Array.isArray(raw.panes)) {
    throw new Error("a recipe needs id, name and panes");
  }
  const preset = PRESETS.includes(raw.preset as never) ? (raw.preset as Recipe["preset"]) : "auto";
  return {
    id: `plugin:${pluginId}:${raw.id}`,
    name: raw.name,
    panes: raw.panes.filter(isObject).map((p) => ({ provider: String(p.provider), prompt: typeof p.prompt === "string" ? p.prompt : undefined })),
    preset,
    url: typeof raw.url === "string" ? raw.url : undefined,
  };
}

const enabledPlugins = () => usePlugins.getState().plugins.filter((p) => p.enabled && !p.error && p.manifest);

/** Tools from every enabled plugin: declared in plugin.json, then registered in code. */
export function pluginTools(): ToolDef[] {
  return enabledPlugins().flatMap((p) => runtimes.get(p.id)?.tools ?? []);
}

/** Pages from every enabled plugin, in plugin order. */
export function pluginPages(): PluginPage[] {
  return enabledPlugins().flatMap((p) => runtimes.get(p.id)?.pages ?? []);
}

const AGENT_TOOL_NAME = /^[A-Za-z0-9_-]{1,64}$/;

/** Agent tools of every enabled plugin; the first plugin to use a name keeps it. */
function agentTools(): { pluginId: string; spec: AgentToolSpec }[] {
  const seen = new Set<string>();
  return enabledPlugins().flatMap((p) =>
    (runtimes.get(p.id)?.agentTools ?? []).flatMap((spec) => {
      if (seen.has(spec.name)) return [];
      seen.add(spec.name);
      return [{ pluginId: p.id, spec }];
    }),
  );
}

/** What `tools/list` adds for agents (MCP tool definitions). */
export function pluginAgentToolList(): { name: string; description: string; inputSchema: Record<string, unknown> }[] {
  return agentTools().map(({ spec }) => ({
    name: spec.name,
    description: spec.description,
    inputSchema: spec.inputSchema ?? { type: "object", properties: {} },
  }));
}

/** Runs an agent's call of a plugin tool; null if no plugin offers `name`. */
export async function runPluginAgentTool(name: string, args: Record<string, unknown>, caller: AgentCaller): Promise<string | null> {
  const found = agentTools().find((t) => t.spec.name === name);
  if (!found) return null;
  return String(await found.spec.run(args, caller));
}

/** Describes the pane `paneId` for plugin agent tools; null if it's gone. */
export function agentCaller(paneId: string): AgentCaller | null {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.panes.some((p) => p.id === paneId));
  const pane = ws?.panes.find((p) => p.id === paneId);
  if (!ws || !pane) return null;
  const position = ws.layout ? leaves(ws.layout).indexOf(pane.id) + 1 : 0;
  return {
    paneId,
    workspaceId: ws.id,
    workspaceName: ws.name,
    cwd: pane.worktree?.path ?? pane.cwd,
    provider: pane.provider,
    label: `${position ? `pane ${position}` : "a pane"} (${providerName(s.providers, pane.provider)})`,
  };
}

/** Recipes from every enabled plugin; invalid ones are skipped. */
export function pluginRecipes(): Recipe[] {
  return enabledPlugins().flatMap((p) =>
    p.manifest!.recipes.flatMap((r) => {
      try {
        return [recipeFromSpec(p.id, r)];
      } catch {
        return [];
      }
    }),
  );
}

/** Runs a panel button's action, reporting failures as toasts. */
export async function runPanelAction(action: PanelAction) {
  try {
    await action.run();
  } catch (err) {
    toast(`Plugin “${action.pluginId}” failed`, "error", String(err));
  }
}

/** Runs `{ type: "plugin.run" }` from the Command Bus. */
export async function runPluginCommand(pluginId: string, command: string, args: Args = {}) {
  const handler = runtimes.get(pluginId)?.commands.get(command);
  if (!handler) {
    toast(`Plugin “${pluginId}” has no command “${command}”`, "error");
    return;
  }
  try {
    await handler(args);
  } catch (err) {
    toast(`Plugin “${pluginId}” failed`, "error", String(err));
  }
}

let dispatch: (cmd: Command) => void = () => {};
/** Set by the Command Bus at startup; avoids an import cycle. */
export function setPluginDispatch(fn: (cmd: Command) => void) {
  dispatch = fn;
}

function makeApi(id: string, dir: string, runtime: Runtime): PluginApi {
  const track = (d: Disposer) => {
    runtime.disposers.push(d);
    return d;
  };
  return {
    id,
    dir,
    appExe: () => invoke<string>("plugins_app_exe"),
    registerTool(spec) {
      const tool = toolFromSpec(id, spec);
      runtime.tools = [...runtime.tools.filter((t) => t.id !== tool.id), tool];
      if (spec.run) runtime.commands.set(`tool:${spec.id}`, spec.run);
      bump();
      return track(() => {
        runtime.tools = runtime.tools.filter((t) => t !== tool);
        if (spec.run) runtime.commands.delete(`tool:${spec.id}`);
        bump();
      });
    },
    registerPage(spec) {
      if (!spec?.id || !spec.title || typeof spec.mount !== "function") throw new Error("a page needs id, title and mount");
      const page: PluginPage = { ...spec, key: `${id}/${spec.id}`, pluginId: id };
      runtime.pages = [...runtime.pages.filter((p) => p.key !== page.key), page];
      bump();
      return track(() => {
        runtime.pages = runtime.pages.filter((p) => p !== page);
        bump();
      });
    },
    registerAgentTool(spec) {
      if (!AGENT_TOOL_NAME.test(spec?.name ?? "")) throw new Error(`agent tool name “${spec?.name}” must be letters, digits, _ or -`);
      if (!spec.description || typeof spec.run !== "function") throw new Error(`agent tool ${spec.name} needs a description and run`);
      runtime.agentTools = [...runtime.agentTools.filter((t) => t.name !== spec.name), spec];
      return track(() => {
        runtime.agentTools = runtime.agentTools.filter((t) => t !== spec);
      });
    },
    registerPanelAction(spec) {
      if (!spec?.id || !spec.title || typeof spec.run !== "function") throw new Error("a panel action needs id, title and run");
      if (!ACTION_PANELS.includes(spec.panel)) throw new Error(`panel action ${spec.id}: panel must be ${ACTION_PANELS.join(" or ")}`);
      const key = `${id}/${spec.id}`;
      const serial = ++panelActionSerial;
      usePlugins.setState((s) => ({ panelActions: [...s.panelActions.filter((a) => a.key !== key), { ...spec, key, pluginId: id, serial }] }));
      return track(() => usePlugins.setState((s) => ({ panelActions: s.panelActions.filter((a) => a.serial !== serial) })));
    },
    updatePanelAction(actionId, look) {
      const key = `${id}/${actionId}`;
      const { title, icon, label, active } = look;
      const patch = Object.fromEntries(Object.entries({ title, icon, label, active }).filter(([k]) => k in look));
      usePlugins.setState((s) => ({ panelActions: s.panelActions.map((a) => (a.key === key ? { ...a, ...patch } : a)) }));
    },
    openPage: (page) => dispatch({ type: "ui.view", view: `page:${id}/${page}` }),
    vault: makeVault(id),
    confirm: (message, opts = {}) => ask(message, { title: opts.title ?? manifestName(id), okLabel: opts.okLabel, kind: opts.kind ?? "warning" }),
    registerCommand(name, handler) {
      runtime.commands.set(name, handler);
      return track(() => runtime.commands.delete(name));
    },
    execute: (cmd) => dispatch(cmd),
    on(type, handler) {
      const listener = (e: PluginEvent) => {
        if (e.type === type) handler(e as never);
      };
      runtime.listeners.add(listener);
      return track(() => runtime.listeners.delete(listener));
    },
    state() {
      const s = useWorkspaces.getState();
      return structuredClone({ workspaces: s.workspaces, activeWorkspaceId: s.activeWorkspaceId, focusedPaneId: s.focusedPaneId });
    },
    describePane: (paneId) => agentCaller(paneId),
    readPane: (paneId, lines = 40) => readPane(paneId, lines),
    readFile: async (path) => new TextDecoder().decode(await invoke<ArrayBuffer>("plugins_read_file", { path })),
    writeFile: (path, data) => writeFile(path, data),
    revealFile: (path) => revealItemInDir(path),
    async paneRequest(paneId) {
      const screen = await readPane(paneId, 40);
      return screen === null ? null : describeRequest(screen);
    },
    lastReply: (paneId) => lastReply(paneId),
    toast: (text, kind = "info") => void toast(text, kind),
  };
}

async function writeFile(path: string, data: string | Uint8Array | ArrayBuffer | Blob): Promise<string> {
  const bytes =
    typeof data === "string" ? new TextEncoder().encode(data) : data instanceof Blob ? new Uint8Array(await data.arrayBuffer()) : new Uint8Array(data);
  // The path rides in a header (ASCII only), so it's base64 of its UTF-8.
  const header = btoa(Array.from(new TextEncoder().encode(path), (b) => String.fromCharCode(b)).join(""));
  return invoke<string>("plugins_write_file", bytes, { headers: { "x-path": header } });
}

async function readPane(paneId: string, lines: number): Promise<string | null> {
  // Imported here: the terminal registry imports the Command Bus, which imports this module.
  const { readTerminal } = await import("@/features/terminal/xtermRegistry");
  return readTerminal(paneId, Math.max(1, Math.min(lines, 2000)));
}

const manifestName = (id: string) => usePlugins.getState().plugins.find((p) => p.id === id)?.manifest?.name ?? id;

function makeVault(plugin: string): PluginVault {
  return {
    list: () => invoke<SecretInfo[]>("vault_list", { plugin }),
    set: (key, value, meta) => invoke<SecretInfo>("vault_set", { plugin, key, value: value ?? null, meta: meta ?? {} }),
    delete: (key) => invoke("vault_delete", { plugin, key }),
    reveal: (key) => invoke<string>("vault_reveal", { plugin, key }),
    exec: ({ command, cwd, keys, timeoutSec }) => invoke("vault_exec", { plugin, command, cwd, keys, timeoutSec: timeoutSec ?? null }),
    async fill({ workspaceId, key, selector, tab }) {
      // Imported here: the browser module imports the Command Bus, which imports this one.
      const { useBrowser } = await import("@/features/browser/browser");
      const b = useBrowser.getState();
      const tabs = b.tabs[workspaceId] ?? [];
      const target = tab ? tabs.find((t) => t.url.includes(tab)) : (tabs.find((t) => t.id === b.activeTab[workspaceId]) ?? tabs[0]);
      if (!target) throw new Error(tab ? `No browser tab matches “${tab}”.` : "The workspace has no browser tab open.");
      await invoke("browser_fill_secret", { workspaceId, targetId: target.id, selector, plugin, key });
      return target.url;
    },
  };
}

/** Imports a plugin's module from its source (it can't be served from disk). */
async function importModule(id: string): Promise<PluginModule> {
  const source = await invoke<string>("plugins_source", { id });
  const url = `data:text/javascript;charset=utf-8,${encodeURIComponent(`${source}\n//# sourceURL=winfer-plugin://${id}`)}`;
  return (await import(/* @vite-ignore */ url)) as PluginModule;
}

async function activate(plugin: PluginInfo) {
  const manifest = plugin.manifest!;
  const runtime: Runtime = { tools: [], pages: [], agentTools: [], commands: new Map(), listeners: new Set(), disposers: [], module: null };
  runtimes.set(plugin.id, runtime);
  setRuntimeError(plugin.id, null);
  const problems: string[] = [];
  for (const raw of manifest.tools) {
    try {
      runtime.tools.push(toolFromSpec(plugin.id, raw));
    } catch (err) {
      problems.push(err instanceof Error ? err.message : String(err));
    }
  }
  bump();
  if (manifest.main) {
    try {
      runtime.module = await importModule(plugin.id);
      // Disabled again while loading.
      if (runtimes.get(plugin.id) !== runtime) return;
      const dispose = await runtime.module.activate?.(makeApi(plugin.id, plugin.dir, runtime));
      if (typeof dispose === "function") runtime.disposers.push(dispose);
    } catch (err) {
      problems.push(`main: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  if (problems.length) setRuntimeError(plugin.id, problems.join("; "));
}

async function deactivate(id: string) {
  const runtime = runtimes.get(id);
  if (!runtime) return;
  runtimes.delete(id);
  for (const dispose of runtime.disposers.reverse()) {
    try {
      dispose();
    } catch {
      // A plugin's cleanup failing shouldn't keep the others from running.
    }
  }
  try {
    await runtime.module?.deactivate?.();
  } catch {
    // Same as above.
  }
  setRuntimeError(id, null);
  bump();
}

/** Rescans the plugin folder and (re)starts the enabled plugins. */
export async function loadPlugins() {
  const plugins = await invoke<PluginInfo[]>("plugins_list");
  for (const id of [...runtimes.keys()]) await deactivate(id);
  usePlugins.setState({ plugins });
  await Promise.all(plugins.filter((p) => p.enabled && !p.error && p.manifest).map(activate));
  // Plugin agents come from Rust's provider list.
  await useWorkspaces.getState().loadProviders();
}

export async function setPluginEnabled(id: string, enabled: boolean) {
  await invoke("plugins_set_enabled", { id, enabled });
  await loadPlugins();
}

export const pluginsDir = () => invoke<string>("plugins_dir");

/** Mirrors `StoreEntry` in src-tauri/src/plugins.rs: a plugin in the repo's plugins/ folder. */
export interface StoreEntry {
  id: string;
  name: string;
  /** Any text (or empty), only compared with the installed one. */
  version: string;
  description: string;
  author?: string | null;
  tags: string[];
}

export const fetchCatalog = () => invoke<StoreEntry[]>("plugins_catalog");

/** Installs or updates a store plugin (it's turned on) and reloads plugins. */
export async function installPlugin(id: string) {
  await invoke("plugins_install", { id });
  await loadPlugins();
}

export async function uninstallPlugin(id: string) {
  await invoke("plugins_uninstall", { id });
  await loadPlugins();
}

/** Matches a search against a plugin's name, id, description and tags (every word must hit). */
export function matchesSearch(query: string, fields: (string | null | undefined)[]): boolean {
  const haystack = fields.filter(Boolean).join(" ").toLowerCase();
  return query.toLowerCase().split(/\s+/).filter(Boolean).every((word) => haystack.includes(word));
}

function emit(event: PluginEvent) {
  for (const runtime of runtimes.values()) {
    for (const listener of runtime.listeners) {
      try {
        listener(event);
      } catch (err) {
        console.error("plugin listener failed", err);
      }
    }
  }
}

/** Feeds pane events to plugins. */
export function startPluginEvents() {
  const panes = (ws: Workspace[]) => new Map<string, Pane>(ws.flatMap((w) => w.panes.map((p) => [p.id, p])));
  let previous = panes(useWorkspaces.getState().workspaces);
  return useWorkspaces.subscribe((s, prev) => {
    if (s.workspaces === prev.workspaces) return;
    // Panes restored at launch aren't news.
    if (!prev.hydrated) {
      previous = panes(s.workspaces);
      return;
    }
    const current = panes(s.workspaces);
    for (const [id, pane] of current) {
      const before = previous.get(id);
      const base = { paneId: id, workspaceId: pane.workspaceId, provider: pane.provider };
      if (!before) emit({ type: "pane.opened", ...base });
      else if (before.status !== pane.status) emit({ type: "pane.status", ...base, status: pane.status, previous: before.status });
    }
    for (const [id, pane] of previous) {
      if (!current.has(id)) emit({ type: "pane.closed", paneId: id, workspaceId: pane.workspaceId, provider: pane.provider });
    }
    previous = current;
  });
}
