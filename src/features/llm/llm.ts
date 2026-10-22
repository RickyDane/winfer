/**
 * The LLM winfer thinks with (prompt writer, progress estimates, review
 * summaries): the user's ChatGPT plan through Sign in with ChatGPT, or their Codex or Claude Code
 * CLI run headless with its own login. Without a ChatGPT sign-in, Codex stands in for it.
 */
import { invoke } from "@tauri-apps/api/core";
import { useWorkspaces } from "@/stores/workspaces";
import { useChatGpt } from "./chatgpt";

export type LlmProvider = "chatgpt" | "codex" | "claude";
export type LlmChoice = LlmProvider | "off";

export const LLM_PROVIDERS: { id: LlmProvider; label: string }[] = [
  { id: "chatgpt", label: "ChatGPT (your plan, Sign in with ChatGPT)" },
  { id: "codex", label: "Codex CLI (your Codex login)" },
  { id: "claude", label: "Claude Code (your Claude login)" },
];

/** Which LLMs can run right now: ChatGPT once signed in, Codex and Claude Code once installed. */
export function availableLlms(): LlmProvider[] {
  const { providers } = useWorkspaces.getState();
  const available: LlmProvider[] = [];
  if (useChatGpt.getState().signedIn) available.push("chatgpt");
  for (const id of ["codex", "claude"] as const) if (providers.some((p) => p.id === id && p.installed)) available.push(id);
  return available;
}

/**
 * The LLM to use: the chosen one if available, else the first that is (so ChatGPT without a
 * sign-in falls back to the Codex CLI); null when off or none is.
 */
export function llmProvider(): LlmProvider | null {
  const choice = useWorkspaces.getState().voiceConfig.writer as string | undefined;
  if (choice === "off") return null;
  const available = availableLlms();
  // Older settings may name a backend that no longer exists; they fall back to what's available.
  return available.find((id) => id === choice) ?? available[0] ?? null;
}

export const llmEnabled = () => llmProvider() !== null;

/**
 * The language to write about a task in, named in the prompt: "the language of the task" alone
 * let models drift, e.g. a terse English task got a Russian progress line.
 */
export function replyLanguage(task: string): string {
  if (/\p{Script=Cyrillic}/u.test(task)) return "the language of the task";
  if (/[äöüß]/i.test(task) || /\b(und|nicht|bitte|ist|das|der|die|mit|für|auf|ich|wir|noch|auch)\b/i.test(task)) return "German";
  return "English";
}

/** True when `text` uses a non-Latin script the task doesn't (the model switched languages). */
export function wrongScript(text: string, task: string): boolean {
  const nonLatin = /[^\P{L}\p{Script=Latin}]/u;
  return nonLatin.test(text) && !nonLatin.test(task);
}

/**
 * One question to the LLM; rejects when it's off or the call fails. `fast` is for tiny jobs
 * (session titles): the backend uses its quickest model.
 */
export async function llmCompose(system: string, user: string, opts?: { fast?: boolean }): Promise<string> {
  const provider = llmProvider();
  if (!provider) throw new Error("No LLM: sign in with ChatGPT or install Codex or Claude Code (Settings → AI)");
  const { writer, writerModel } = useWorkspaces.getState().voiceConfig;
  // A model picked for the other backend doesn't apply.
  const model = writer === provider ? (writerModel ?? "") : "";
  return invoke<string>("llm_compose", { req: { provider, model, system, user, ...(opts?.fast ? { fast: true } : {}) } });
}
