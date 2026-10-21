/**
 * Everything live voice can do, as Realtime API function tools: the Tool Registry (winfer's UI
 * actions, incl. plugin ones), the agent tools agents get over MCP (read/send/spawn panes, open
 * URLs and files, usage), plugin agent tools, background tasks, and — once asked for — the
 * agent browser's Playwright tools.
 */
import { invoke } from "@tauri-apps/api/core";
import { execute } from "@/commands/bus";
import { toolContext } from "@/features/palette/entries";
import { pluginAgentToolList } from "@/features/plugins/plugins";
import { runVoiceTool } from "@/features/mcp/agentApi";
import { hiddenToolsOf, useWorkspaces } from "@/stores/workspaces";
import { allTools, type ParamDef, type ToolContext, type ToolDef } from "@/tools/registry";
import { decisionCommands } from "../decision";
import { validArgs } from "../planner";

export interface RealtimeTool {
  type: "function";
  name: string;
  description: string;
  parameters: Record<string, unknown>;
}

type JsonSchema = Record<string, unknown>;

const PANE = { type: ["integer", "string"], description: "Pane number as listed (1, 2, …) or pane id" };

/** The MCP agent tools, described for voice (no calling pane: "the user's workspace"). */
const AGENT_TOOLS: RealtimeTool[] = [
  {
    name: "list_panes",
    description: "List the panes in the current workspace: number, agent, status (working, needs-input, done, idle, error), branch, which is focused.",
    parameters: { type: "object", properties: {} },
  },
  {
    name: "read_pane",
    description: "Read the latest terminal output of a pane, e.g. to tell the user what an agent did, is doing or is asking.",
    parameters: { type: "object", properties: { pane: PANE, lines: { type: "integer", description: "Lines from the end (default 60)" } }, required: ["pane"] },
  },
  {
    name: "send_to_pane",
    description: "Type a message into a pane, e.g. an instruction for an agent or a command in a shell pane. Submits unless submit is false. Write the message as the agent should receive it.",
    parameters: { type: "object", properties: { pane: PANE, text: { type: "string" }, submit: { type: "boolean" } }, required: ["pane", "text"] },
  },
  {
    name: "spawn_agent",
    description: "Start one new agent pane, optionally with a first prompt and its own git worktree. Providers: the installed agent ids, or shell.",
    parameters: { type: "object", properties: { provider: { type: "string" }, prompt: { type: "string" }, worktree: { type: "boolean" } }, required: ["provider"] },
  },
  {
    name: "open_url",
    description: "Open a URL in winfer's browser panel (reuses a tab already on that site unless new_tab).",
    parameters: { type: "object", properties: { url: { type: "string" }, new_tab: { type: "boolean" } }, required: ["url"] },
  },
  {
    name: "open_file",
    description: "Show a file in winfer's file viewer (markdown rendered, images shown). Relative paths start at the workspace folder.",
    parameters: { type: "object", properties: { path: { type: "string" }, line: { type: "number" } }, required: ["path"] },
  },
  {
    name: "usage_status",
    description: "How much of an agent's plan usage is left (5-hour and weekly windows, resets). Defaults to the focused pane's agent.",
    parameters: { type: "object", properties: { pane: PANE } },
  },
].map((t) => ({ type: "function" as const, ...t }));

/** Registry tools the agent tools above replace. */
const REPLACED = new Set(["send_to_pane", "open_url", "browser_new_tab", "browser_navigate", "repeat_last", "live_voice"]);

/** Tools the model must confirm with the user before running. */
const DESTRUCTIVE = new Set(["close_pane", "close_all_panes", "discard_worktree", "merge_worktree", "interrupt_all"]);

export const USE_BROWSER = "use_browser";
/** Handled by the session: ends the conversation. */
export const END_CONVERSATION = "end_conversation";
const END_TOOL: RealtimeTool = {
  type: "function",
  name: END_CONVERSATION,
  description: "End the live voice conversation when the user says goodbye or asks you to stop listening.",
  parameters: { type: "object", properties: {} },
};
const WEB = "web_";
const BG = "bg";
const MAX_RESULT = 4000;

function paramSchema(param: ParamDef, ctx: ToolContext): JsonSchema {
  const described = (options: Record<string, string | null>) =>
    Object.entries(options)
      .map(([id, label]) => (label && label !== id ? `${id} = ${label}` : id))
      .join("; ");
  switch (param.kind) {
    case "enum": {
      const options = param.options(ctx);
      return { type: "string", enum: Object.keys(options), description: `${param.ask} (${described(options)})` };
    }
    case "perOption": {
      const options = param.options(ctx);
      return {
        type: "object",
        description: `${param.ask} Counts 0–${param.max} keyed by: ${described(options)}`,
        properties: Object.fromEntries(Object.keys(options).map((o) => [o, { type: "integer", minimum: 0, maximum: param.max }])),
      };
    }
    case "count":
      return { type: "integer", minimum: 0, maximum: param.max, description: param.ask };
    case "bool":
      return { type: "boolean", description: param.ask };
    case "level":
      return { type: "string", enum: param.levels, description: param.ask };
    case "span":
      return { type: "string", description: param.ask };
  }
}

const destructive = (tool: ToolDef) => tool.risk === "confirm" || DESTRUCTIVE.has(tool.id);

export function registryTool(tool: ToolDef, ctx: ToolContext): RealtimeTool {
  const properties: Record<string, JsonSchema> = Object.fromEntries(Object.entries(tool.params).map(([name, p]) => [name, paramSchema(p, ctx)]));
  if (tool.id === "spawn_panes") properties.worktree = { type: "boolean", description: "Start each new agent in its own git worktree and branch" };
  if (destructive(tool)) properties.confirmed = { type: "boolean", description: "True only after the user said yes to doing this" };
  return {
    type: "function",
    name: tool.id,
    description: destructive(tool) ? `${tool.description} Ask the user to confirm first.` : tool.description,
    parameters: { type: "object", properties },
  };
}

/** Playwright's tools as `web_*`, minus those the user turned off for agents. */
export function webTools(mcpTools: { name: string; description?: string; inputSchema?: JsonSchema }[], hidden: string[]): RealtimeTool[] {
  return mcpTools
    .filter((t) => !hidden.includes(t.name))
    .map((t) => ({
      type: "function",
      name: WEB + t.name.replace(/^browser_/, ""),
      description: (t.description ?? "").split("\n")[0].slice(0, 300),
      parameters: t.inputSchema ?? { type: "object", properties: {} },
    }));
}

let browserTools: { workspaceId: string; tools: RealtimeTool[] } | null = null;
let bgTool: RealtimeTool | null = null;

/** The tools for the current app state. */
export async function voiceTools(): Promise<RealtimeTool[]> {
  const ctx = toolContext();
  const registry = allTools()
    .filter((t) => !REPLACED.has(t.id))
    .map((t) => registryTool(t, ctx));
  const taken = new Set([...AGENT_TOOLS, ...registry].map((t) => t.name));
  const plugin = pluginAgentToolList()
    .filter((t) => !taken.has(t.name))
    .map((t): RealtimeTool => ({ type: "function", name: t.name, description: t.description.slice(0, 400), parameters: t.inputSchema }));
  if (!bgTool) {
    const spec = await invoke<{ description: string; inputSchema: JsonSchema }>("realtime_bg_tool").catch(() => null);
    if (spec) bgTool = { type: "function", name: BG, description: `Run a long-running command (dev server, watcher, build) in the workspace folder in the background. ${spec.description.split(". ")[0]}.`, parameters: spec.inputSchema };
  }
  const browser =
    browserTools && browserTools.workspaceId === ctx.active.id
      ? browserTools.tools
      : [{ type: "function" as const, name: USE_BROWSER, description: "Load the tools to see and control web pages in winfer's browser (snapshot, click, type, fill forms, console). Call before acting on a page.", parameters: { type: "object", properties: {} } }];
  return [...AGENT_TOOLS, ...registry, ...plugin, ...(bgTool ? [bgTool] : []), ...browser, END_TOOL];
}

const truncate = (text: string) => (text.length > MAX_RESULT ? `${text.slice(0, MAX_RESULT)}\n… (truncated)` : text);

/**
 * Runs one function call; returns the text for the model. `onToolsChanged` is called when the
 * available tools changed (browser tools loaded), so the session can update them.
 */
export async function runVoiceFunction(name: string, args: Record<string, unknown>, onToolsChanged: () => void): Promise<string> {
  try {
    return truncate(await dispatch(name, args, onToolsChanged));
  } catch (err) {
    return `Error: ${err instanceof Error ? err.message : String(err)}`;
  }
}

async function dispatch(name: string, args: Record<string, unknown>, onToolsChanged: () => void): Promise<string> {
  const s = useWorkspaces.getState();
  const workspaceId = s.activeWorkspaceId;
  if (name === USE_BROWSER) {
    const list = await invoke<{ name: string; description?: string; inputSchema?: JsonSchema }[]>("realtime_browser_tools", { workspaceId });
    browserTools = { workspaceId, tools: webTools(list, hiddenToolsOf(s)) };
    onToolsChanged();
    return `Browser tools ready: ${browserTools.tools.map((t) => t.name).join(", ")}. Take a web_snapshot to see the page and get element refs.`;
  }
  if (name.startsWith(WEB)) {
    if (!browserTools || browserTools.workspaceId !== workspaceId) return `Call ${USE_BROWSER} first.`;
    const reply = await invoke<{ ok: boolean; text: string }>("realtime_browser_call", { workspaceId, name: "browser_" + name.slice(WEB.length), args });
    return reply.ok ? reply.text || "Done." : `Error: ${reply.text}`;
  }
  if (name === BG) {
    const ws = s.workspaces.find((w) => w.id === workspaceId);
    const reply = await invoke<{ ok: boolean; text: string }>("realtime_bg", { args, cwd: ws?.rootDir ?? "" });
    return reply.ok ? reply.text : `Error: ${reply.text}`;
  }
  if (AGENT_TOOLS.some((t) => t.name === name)) {
    if (name === "read_pane" && args.lines === undefined) args = { ...args, lines: 60 };
    return runVoiceTool(name, args);
  }
  const tool = allTools().find((t) => t.id === name);
  if (tool) {
    if (destructive(tool) && args.confirmed !== true) return "Not done: ask the user to confirm this first, then call again with confirmed: true.";
    const ctx = toolContext();
    const commands = decisionCommands({ tool, args: validArgs(tool, args, ctx) }, ctx);
    if (commands.length === 0) return "Nothing to do with those arguments.";
    commands.forEach(execute);
    return "Done.";
  }
  // Plugin agent tools (secrets, recorder, task board, …).
  return runVoiceTool(name, args);
}

/** Forget loaded browser tools (session ended). */
export function resetVoiceTools() {
  browserTools = null;
}
