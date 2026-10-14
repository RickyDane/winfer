/**
 * Plugins: folders in the app data dir with a `plugin.json` (see docs/plugins.md and
 * src-tauri/src/plugins.rs). A plugin contributes any of:
 *
 * - `providers`: agent CLIs (handled in Rust; they show up in `providers_list`).
 * - `recipes`: agent setups, listed beside the user's own.
 * - `tools`: declarative actions for the palette and voice that run Command Bus commands.
 * - `main`: an ES module whose `activate(api)` registers tools and command handlers in code.
 *
 * Plugins start disabled; the user turns each one on in Settings.
 */
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { Command } from "@/commands/bus";
import type { Args, ParamDef, ToolDef } from "@/tools/registry";
import type { Recipe } from "@/features/recipes/recipes";
import { PRESETS } from "@/features/layout/tree";
import { toast } from "@/stores/toasts";
import { useWorkspaces, type AgentStatus, type Pane, type Workspace } from "@/stores/workspaces";

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

/** What a plugin's `activate` receives. */
export interface PluginApi {
  readonly id: string;
  registerTool(spec: ToolSpec): Disposer;
  /** Handles `{ type: "plugin.run", plugin: <this id>, command: name, args }`. */
  registerCommand(name: string, handler: (args: Args) => void | Promise<void>): Disposer;
  /** Runs any Command Bus command. */
  execute(command: Command): void;
  on<T extends PluginEvent["type"]>(type: T, handler: (event: Extract<PluginEvent, { type: T }>) => void): Disposer;
  /** A read-only snapshot of workspaces and panes. */
  state(): { workspaces: Workspace[]; activeWorkspaceId: string; focusedPaneId: string | null };
  toast(text: string, kind?: "info" | "success" | "error"): void;
}

interface PluginModule {
  activate?: (api: PluginApi) => void | Disposer | Promise<void | Disposer>;
  deactivate?: () => void | Promise<void>;
}

interface Runtime {
  tools: ToolDef[];
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
}

export const usePlugins = create<PluginsState>(() => ({ plugins: [], runtimeErrors: {}, version: 0 }));

const runtimes = new Map<string, Runtime>();
const bump = () => usePlugins.setState((s) => ({ version: s.version + 1 }));

const setRuntimeError = (id: string, error: string | null) =>
  usePlugins.setState((s) => {
    const runtimeErrors = { ...s.runtimeErrors };
    if (error) runtimeErrors[id] = error;
    else delete runtimeErrors[id];
    return { runtimeErrors };
  });

/** Usable ids for the decision API, which only accepts [A-Za-z0-9_-]. */
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

function makeApi(id: string, runtime: Runtime): PluginApi {
  const track = (d: Disposer) => {
    runtime.disposers.push(d);
    return d;
  };
  return {
    id,
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
    toast: (text, kind = "info") => void toast(text, kind),
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
  const runtime: Runtime = { tools: [], commands: new Map(), listeners: new Set(), disposers: [], module: null };
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
      const dispose = await runtime.module.activate?.(makeApi(plugin.id, runtime));
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
