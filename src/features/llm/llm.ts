/**
 * The LLM winfer thinks with (voice planner, prompt writer, progress estimates, review
 * summaries): the user's ChatGPT plan through Sign in with ChatGPT, or their Claude Code CLI run
 * headless with its own login.
 */
import { invoke } from "@tauri-apps/api/core";
import { useWorkspaces } from "@/stores/workspaces";
import { useChatGpt } from "./chatgpt";

export type LlmProvider = "chatgpt" | "claude";
export type LlmChoice = LlmProvider | "off";

export const LLM_PROVIDERS: { id: LlmProvider; label: string }[] = [
  { id: "chatgpt", label: "ChatGPT (your plan, Sign in with ChatGPT)" },
  { id: "claude", label: "Claude Code (your Claude login)" },
];

/** Which LLMs can run right now: ChatGPT once signed in, Claude Code once installed. */
export function availableLlms(): LlmProvider[] {
  const { providers } = useWorkspaces.getState();
  const available: LlmProvider[] = [];
  if (useChatGpt.getState().signedIn) available.push("chatgpt");
  if (providers.some((p) => p.id === "claude" && p.installed)) available.push("claude");
  return available;
}

/** The LLM to use: the chosen one if available, else the first that is; null when off or none is. */
export function llmProvider(): LlmProvider | null {
  const choice = useWorkspaces.getState().voiceConfig.writer as string | undefined;
  if (choice === "off") return null;
  const available = availableLlms();
  // Older settings may name a backend that no longer exists; they fall back to what's available.
  return available.find((id) => id === choice) ?? available[0] ?? null;
}

export const llmEnabled = () => llmProvider() !== null;

/** One question to the LLM; rejects when it's off or the call fails. */
export async function llmCompose(system: string, user: string): Promise<string> {
  const provider = llmProvider();
  if (!provider) throw new Error("No LLM: sign in with ChatGPT or install Claude Code (Settings → Voice)");
  const { writer, writerModel } = useWorkspaces.getState().voiceConfig;
  // A model picked for the other backend doesn't apply.
  const model = writer === provider ? (writerModel ?? "") : "";
  return invoke<string>("llm_compose", { req: { provider, model, system, user } });
}
