/**
 * The prompt writer: an LLM turns what the user said into the message an agent receives,
 * using context (the agent, the workspace, the agent's recent output). Clef already chose the
 * action; this only rewrites its instruction text. Falls back to the spoken words on failure.
 */
import { invoke } from "@tauri-apps/api/core";
import { leaves } from "@/features/layout/tree";
import { readTerminal } from "@/features/terminal/xtermRegistry";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import type { Command } from "@/commands/bus";
import type { ProviderId } from "@/features/providers/providers";
import type { Decision } from "./decision";

export type WriterProvider = "off" | "workers" | "codex";

export const WORKERS_MODELS = [
  { id: "@cf/openai/gpt-oss-20b", label: "GPT-OSS 20B (fastest)" },
  { id: "@cf/meta/llama-3.3-70b-instruct-fp8-fast", label: "Llama 3.3 70B" },
  { id: "@cf/qwen/qwen3-30b-a3b-fp8", label: "Qwen3 30B" },
  { id: "@cf/google/gemma-4-26b-a4b-it", label: "Gemma 4 26B" },
];

const SYSTEM = [
  "You write the exact message a coding agent (Claude Code, Codex, …) will receive, from what the user said by voice.",
  "Use the context (agent, folder, its recent output) to be concrete: name files, pages and errors it mentioned.",
  "Keep the user's intent; don't add requirements they didn't ask for. Write in the user's language.",
  "1–4 sentences. Output only the message, no quotes or preamble.",
].join(" ");

/** Which argument of a decision holds its instruction text, if it should be written. */
function instructionField(d: Decision): string | null {
  for (const [name, param] of Object.entries(d.tool.params)) {
    if (param.kind === "span" && param.slot === "instruction" && typeof d.args[name] === "string") {
      // "Type hello world into Claude": typed literally, not rewritten.
      if (d.tool.id === "send_to_pane" && d.args.submit === false) return null;
      return name;
    }
  }
  return null;
}

function recipients(d: Decision): { names: string; panes: Pane[] } {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId)!;
  const name = (id: string) => providerName(s.providers, id);
  if (d.tool.id === "send_to_pane") {
    const id = d.args.target && d.args.target !== "focused" && d.args.target !== "all" ? String(d.args.target) : s.focusedPaneId;
    const pane = ws.panes.find((p) => p.id === id);
    return pane ? { names: name(pane.provider), panes: [pane] } : { names: "an agent", panes: [] };
  }
  if (d.tool.id === "spawn_panes" || d.tool.id === "race_agents") {
    const counts = (d.args.counts ?? {}) as Record<string, number>;
    return { names: Object.keys(counts).filter((p) => counts[p] > 0).map(name).join(", ") || "new agents", panes: [] };
  }
  const order = ws.layout ? leaves(ws.layout) : [];
  return { names: "all agents in the workspace", panes: order.map((id) => ws.panes.find((p) => p.id === id)!).slice(0, 2) };
}

function context(d: Decision, transcript: string, instruction: string): string {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId)!;
  const { names, panes } = recipients(d);
  const parts = [
    `Spoken: "${transcript}"`,
    `Instruction as heard: "${instruction}"`,
    `Recipient: ${names}${d.tool.id === "spawn_panes" ? " (new agent, starting fresh)" : ""}`,
    `Workspace: ${ws.name} (${ws.rootDir})`,
  ];
  for (const pane of panes) {
    const output = (readTerminal(pane.id, 80) ?? "").slice(-2500).trim();
    if (output) parts.push(`Recent output of ${providerName(s.providers, pane.provider)} in ${pane.cwd}:\n${output}`);
  }
  return parts.join("\n");
}

export function writerEnabled(): boolean {
  const provider = useWorkspaces.getState().voiceConfig.writer ?? "workers";
  return provider !== "off";
}

/** Rewrites the instruction of each decision that has one. */
export async function writeInstructions(decisions: Decision[], transcript: string): Promise<Decision[]> {
  const config = useWorkspaces.getState().voiceConfig;
  const provider = config.writer ?? "workers";
  if (provider === "off") return decisions;
  return Promise.all(
    decisions.map(async (d) => {
      const field = instructionField(d);
      if (!field) return d;
      try {
        const text = await invoke<string>("llm_compose", {
          req: { provider, model: config.writerModel ?? "", system: SYSTEM, user: context(d, transcript, String(d.args[field])) },
        });
        return { ...d, args: { ...d.args, [field]: text } };
      } catch (err) {
        console.warn("prompt writer failed, using the spoken words", err);
        return d;
      }
    }),
  );
}

/** Whether any decision has an instruction the writer would rewrite. */
export const needsWriting = (decisions: Decision[]) => writerEnabled() && decisions.some((d) => instructionField(d) !== null);

// ---------------------------------------------------------------------------------------------
// Per-pane planning: one command, several recipients, possibly different instructions
// ("all of you build Flappy Bird, except the last one: Super Mario").

export interface Recipient {
  /** Shown to the LLM, e.g. "pane 2 (Codex)" or "new agent 3 (Claude Code)". */
  label: string;
  /** Builds the command that delivers this recipient's message. */
  deliver: (message: string) => Command;
}

/** Recipients of a decision that reaches several agents, or null for single-recipient ones. */
export function recipientsOf(d: Decision, spawnOrder: ProviderId[]): Recipient[] | null {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId)!;
  const name = (id: string) => providerName(s.providers, id);
  const order = ws.layout ? leaves(ws.layout) : [];
  const agents = order.map((id, i) => ({ pane: ws.panes.find((p) => p.id === id)!, n: i + 1 })).filter((x) => x.pane.provider !== "shell");

  if (d.tool.id === "broadcast" || (d.tool.id === "send_to_pane" && d.args.target === "all")) {
    const submit = d.args.submit !== false;
    return agents.length > 1
      ? agents.map(({ pane, n }) => ({
          label: `pane ${n} (${name(pane.provider)})`,
          deliver: (text) => ({ type: "pane.sendText", target: { id: pane.id }, text, submit }),
        }))
      : null;
  }
  if (d.tool.id === "spawn_panes" && d.args.prompt) {
    const model = d.args.model && d.args.model !== "default" ? String(d.args.model) : undefined;
    return spawnOrder.length > 1
      ? spawnOrder.map((provider, i) => ({
          label: `new agent ${i + 1} (${name(provider)})`,
          deliver: (prompt) => ({ type: "pane.spawn", provider, count: 1, prompt, model }),
        }))
      : null;
  }
  return null;
}

export const PLAN_SYSTEM = [
  "The user gave one voice command that goes to several coding agents. Decide the exact message each recipient receives.",
  "Recipients can get different tasks (e.g. 'all build X except the last one, which builds Y'). 'The last one' is the last in the list.",
  "Each message must stand alone: the agent doesn't know about the others or the list. Be concrete, 1–3 sentences, in the user's language.",
  'Output only JSON: an array with exactly one string per recipient, in list order, e.g. ["…", "…"].',
].join(" ");

/** Parses the first JSON string array in the model's answer; with `expected`, it must have that many. */
export function parseMessages(answer: string, expected?: number): string[] | null {
  const start = answer.indexOf("[");
  const end = answer.lastIndexOf("]");
  if (start < 0 || end <= start) return null;
  try {
    const list = JSON.parse(answer.slice(start, end + 1));
    return Array.isArray(list) && list.length > 0 && (expected === undefined || list.length === expected) && list.every((m) => typeof m === "string" && m.trim())
      ? list.map((m: string) => m.trim())
      : null;
  } catch {
    return null;
  }
}

/** Asks the LLM for one message per recipient; null if the writer is off or the plan doesn't fit. */
export async function planPerRecipient(transcript: string, instruction: string, recipients: Recipient[]): Promise<Command[] | null> {
  const config = useWorkspaces.getState().voiceConfig;
  const provider = config.writer ?? "workers";
  if (provider === "off") return null;
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId)!;
  const user = [
    `Spoken: "${transcript}"`,
    `Instruction as heard: "${instruction}"`,
    `Workspace: ${ws.name} (${ws.rootDir})`,
    `Recipients, in order:\n${recipients.map((r, i) => `${i + 1}. ${r.label}`).join("\n")}`,
  ].join("\n");
  try {
    const answer = await invoke<string>("llm_compose", { req: { provider, model: config.writerModel ?? "", system: PLAN_SYSTEM, user } });
    const messages = parseMessages(answer, recipients.length);
    return messages ? recipients.map((r, i) => r.deliver(messages[i])) : null;
  } catch (err) {
    console.warn("per-pane planning failed", err);
    return null;
  }
}

// ---------------------------------------------------------------------------------------------
// Task splitting: "create X, implement Y and change Z" for one new agent becomes one agent per
// feature, each in its own worktree, so the work can be reviewed and merged separately.

export const SPLIT_SYSTEM = [
  "The user gave a voice command that starts a new coding agent. Split it into separate tasks: each task goes to its own agent,",
  "working in its own git worktree, and is merged later through its own pull request.",
  "When the user lists several features or changes (separate sentences, files or components), make one task per feature or change,",
  "even if one later uses another. Keep it as one task only when it is a single feature, when the user asks for one agent to do everything,",
  "Follow-up steps of a change (run the tests, check it, commit it) belong to that change's task, never a task of their own.",
  "Instructions about winfer itself ('start it in Claude Code', 'open Codex') are not tasks; leave them out.",
  "Each message must stand alone: the agent doesn't know about the others. Only when there are several tasks and one uses something another creates",
  "(a file, a function), say what it can expect and to create a minimal version if it doesn't exist yet.",
  "Keep the user's intent; don't add requirements they didn't ask for. Be concrete, 1–4 sentences each, in the user's language.",
  'Output only JSON: an array with one string per task, e.g. ["…", "…"].',
].join(" ");

/** Splits an instruction into one message per separate task; null if the writer is off or fails. */
export async function splitTasks(transcript: string, instruction: string, agent: string): Promise<string[] | null> {
  const config = useWorkspaces.getState().voiceConfig;
  const provider = config.writer ?? "workers";
  if (provider === "off") return null;
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId)!;
  const user = [
    `Spoken: "${transcript}"`,
    `Instruction as heard: "${instruction}"`,
    `Agent: ${agent} (new agent, starting fresh)`,
    `Workspace: ${ws.name} (${ws.rootDir})`,
  ].join("\n");
  try {
    const answer = await invoke<string>("llm_compose", { req: { provider, model: config.writerModel ?? "", system: SPLIT_SYSTEM, user } });
    return parseMessages(answer)?.slice(0, 6) ?? null;
  } catch (err) {
    console.warn("task splitting failed", err);
    return null;
  }
}
