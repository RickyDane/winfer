/**
 * The prompt writer: an LLM turns what the user said into the message an agent receives,
 * using context (the agent, the workspace, the agent's recent output). Clef already chose the
 * action; this only rewrites its instruction text. Falls back to the spoken words on failure.
 */
import { invoke } from "@tauri-apps/api/core";
import { leaves } from "@/features/layout/tree";
import { readTerminal } from "@/features/terminal/xtermRegistry";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
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
