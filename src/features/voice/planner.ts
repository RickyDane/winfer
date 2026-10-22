/**
 * The LLM planner: every voice command is planned by the LLM (ChatGPT, Codex or Claude Code, see
 * llm.ts).
 *
 * It sees the Tool Registry (ids, descriptions, allowed values) plus the app state, and answers
 * with a JSON plan of tool calls whose prompts it writes itself. Every call is validated against
 * the registry, so it can still only run what winfer offers.
 */
import { llmCompose, llmEnabled } from "@/features/llm/llm";
import { leaves } from "@/features/layout/tree";
import { readTerminal } from "@/features/terminal/xtermRegistry";
import { providerName } from "@/stores/workspaces";
import { allTools, type Args, type ParamDef, type ToolContext, type ToolDef } from "@/tools/registry";
import type { Decision } from "./decision";

export const PLANNER_SYSTEM = `You control winfer, a desktop app that runs coding agents (Claude Code, Codex, …) in terminal panes, from the user's voice command.
Decide which winfer tools to call and with which arguments. Only use the tools and allowed values listed; leave out arguments that don't apply.

Rules:
- Do only what the user asked. Never add actions of your own (no extra messages, dev servers or test runs).
- "Open", "start" or "launch" an agent means a NEW agent (spawn_panes), even if one of that kind is already open. New work that doesn't address an open pane also means new agents.
- send_to_pane only when the user talks to an agent that is already open ("tell Claude …", "the Codex pane", "ask it …"). Never send prompts to Shell panes.
- Naming an agent for the work ("in Claude Code", "mit Codex") only picks which agent the new panes run. It is never a message or a task.
- "Start/run the project in <agent>" ("Starte das Projekt in Claude Code") after a list of tasks means: do those tasks with <agent>. It adds no action of its own.
- Several separate features or changes: one spawn_panes call per feature, each with count 1 and "worktree": true, so each gets its own branch and pull request. If the user says how many features there are, make exactly that many. Each sentence or list item asking for a different file, component or feature is its own change, even when they build on each other.
- One task with follow-up steps ("fix it, then run the tests") is one agent with one prompt.
- Several agents on the same task to compare them: race_agents.
- Prompts and messages are what the agent receives: clear, concrete instructions in the language the user spoke, standing alone (an agent doesn't know about the others). Keep the user's intent; don't add requirements. No lead-ins like "tell it to".
- Names may be misheard: cloud means Claude, codecs means Codex. If no agent is named for new work, use the default agent.
- Speech that isn't a request to winfer (chit-chat, background talk): no actions.

Examples (with a Claude Code pane and a Codex pane open):
- "Open Codex and have it fix the flaky upload test" → [spawn_panes {counts: {codex: 1}, prompt: "Fix the flaky upload test."}]
- "Baue einen Dark Mode und eine Suchleiste. Starte das anschließend in Claude Code." → [spawn_panes {counts: {claude: 1}, prompt: "Baue einen Dark Mode …", worktree: true}, spawn_panes {counts: {claude: 1}, prompt: "Baue eine Suchleiste …", worktree: true}]
- "Tell Codex to also update the changelog" → [send_to_pane {target: <the Codex pane id>, text: "Also update the changelog.", submit: true}]

Answer only with JSON: {"actions": [{"tool": "<tool id>", "args": {…}}]}.`;

function describeParam(name: string, param: ParamDef, ctx: ToolContext): string {
  const values = (options: Record<string, string | null>) =>
    Object.entries(options)
      .map(([id, label]) => (label && label !== id ? `"${id}" (${label})` : `"${id}"`))
      .join(", ");
  switch (param.kind) {
    case "enum":
      return `${name}: one of ${values(param.options(ctx))} — ${param.ask}`;
    case "perOption":
      return `${name}: object of counts 0–${param.max} keyed by ${values(param.options(ctx))} — ${param.ask}`;
    case "count":
      return `${name}: number 0–${param.max} — ${param.ask}`;
    case "bool":
      return `${name}: true or false — ${param.ask}`;
    case "level":
      return `${name}: one of ${param.levels.map((l) => `"${l}"`).join(", ")} — ${param.ask}`;
    case "span":
      return `${name}: text — ${param.ask}`;
  }
}

function describeTool(tool: ToolDef, ctx: ToolContext): string {
  const params = Object.entries(tool.params).map(([name, p]) => `    ${describeParam(name, p, ctx)}`);
  if (tool.id === "spawn_panes") params.push('    worktree: true or false — start each new agent in its own git worktree and branch');
  return [`- ${tool.id}: ${tool.description}`, ...params].join("\n");
}

/** The user message: app state, recent output of the focused agent, the tool catalog and the command. */
export function plannerPrompt(transcript: string, ctx: ToolContext, recent: string[] = [], focusedOutput = ""): string {
  const order = ctx.active.layout ? leaves(ctx.active.layout) : [];
  const panes = order.map((id, i) => {
    const pane = ctx.active.panes.find((p) => p.id === id)!;
    const focused = id === ctx.focusedPaneId ? ", focused" : "";
    return `  pane ${i + 1} (id "${id}"): ${providerName(ctx.providers, pane.provider)}, ${pane.status}${pane.worktree ? `, branch ${pane.worktree.branch}` : ""}${focused}`;
  });
  const parts = [
    `Workspace: ${ctx.active.name} (${ctx.active.rootDir})`,
    `Open panes:\n${panes.join("\n") || "  none"}`,
    `Other workspaces: ${ctx.workspaces.filter((w) => w.id !== ctx.active.id).map((w) => w.name).join(", ") || "none"}`,
    `Installed agents: ${ctx.providers.filter((p) => p.installed).map((p) => `${p.name} ("${p.id}")`).join(", ")}`,
    `Browser tabs: ${ctx.browserTabs.join(", ") || "none"}`,
  ];
  if (recent.length) parts.push(`Recent voice commands:\n${recent.map((r) => `  ${r}`).join("\n")}`);
  if (focusedOutput) parts.push(`Recent output of the focused pane:\n${focusedOutput}`);
  parts.push(`Tools:\n${allTools().map((t) => describeTool(t, ctx)).join("\n")}`);
  parts.push(`Voice command: "${transcript}"`);
  return parts.join("\n\n");
}

/** Keeps only arguments the tool accepts, with values it allows. */
export function validArgs(tool: ToolDef, raw: Record<string, unknown>, ctx: ToolContext): Args {
  const args: Args = {};
  for (const [name, param] of Object.entries(tool.params)) {
    const value = raw[name];
    switch (param.kind) {
      case "enum": {
        const options = Object.keys(param.options(ctx));
        if (typeof value === "string" && options.includes(value)) args[name] = value;
        else if (options.length === 1) args[name] = options[0];
        break;
      }
      case "perOption": {
        const options = Object.keys(param.options(ctx));
        const given = value && typeof value === "object" ? (value as Record<string, unknown>) : {};
        args[name] = Object.fromEntries(options.map((o) => [o, clampCount(given[o], param.max)]));
        break;
      }
      case "count":
        if (value !== undefined) args[name] = clampCount(value, param.max);
        break;
      case "bool":
        if (typeof value === "boolean") args[name] = value;
        break;
      case "level":
        if (typeof value === "string" && param.levels.includes(value)) args[name] = value;
        break;
      case "span":
        if (typeof value === "string" && value.trim()) args[name] = value.trim();
        break;
    }
  }
  if (tool.id === "spawn_panes" && raw.worktree === true) args.worktree = true;
  return args;
}

function clampCount(value: unknown, max: number): number {
  const n = Math.round(Number(value));
  return Number.isFinite(n) ? Math.min(Math.max(n, 0), max) : 0;
}

/** Parses the planner's answer into decisions; null if it isn't a usable plan. */
export function parsePlan(answer: string, ctx: ToolContext): Decision[] | null {
  const start = answer.indexOf("{");
  const end = answer.lastIndexOf("}");
  if (start < 0 || end <= start) return null;
  let plan: unknown;
  try {
    plan = JSON.parse(answer.slice(start, end + 1));
  } catch {
    return null;
  }
  const actions = (plan as { actions?: unknown })?.actions;
  if (!Array.isArray(actions)) return null;
  return actions.flatMap((action) => {
    const id = (action as { tool?: unknown })?.tool;
    const tool = allTools().find((t) => t.id === id);
    if (!tool) return [];
    const raw = (action as { args?: unknown }).args;
    const args = validArgs(tool, raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {}, ctx);
    return [{ tool, args }];
  });
}

/** Asks the LLM for a plan; null when the LLM is off or fails. */
export async function planWithLlm(transcript: string, ctx: ToolContext, recent: string[]): Promise<Decision[] | null> {
  if (!llmEnabled()) return null;
  const focusedOutput = ctx.focusedPaneId ? (readTerminal(ctx.focusedPaneId, 40) ?? "").slice(-2000).trim() : "";
  try {
    const answer = await llmCompose(PLANNER_SYSTEM, plannerPrompt(transcript, ctx, recent, focusedOutput));
    return parsePlan(answer, ctx);
  } catch (err) {
    console.warn("LLM planner failed", err);
    return null;
  }
}
