/**
 * The LLM winfer thinks with (voice planner, prompt writer, progress estimates, review
 * summaries): the user's own Claude Code or Codex CLI, run headless with its own login.
 */
import { invoke } from "@tauri-apps/api/core";
import { useWorkspaces } from "@/stores/workspaces";

export type LlmProvider = "claude" | "codex";
export type LlmChoice = LlmProvider | "off";

export const LLM_PROVIDERS: { id: LlmProvider; label: string }[] = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
];

/** The CLI to use: the chosen one if installed, else whichever is; null when off or none is installed. */
export function llmProvider(): LlmProvider | null {
  const { voiceConfig, providers } = useWorkspaces.getState();
  const choice = voiceConfig.writer as string | undefined;
  if (choice === "off") return null;
  // Before the provider list has loaded, trust the choice.
  if (providers.length === 0) return choice === "codex" ? "codex" : "claude";
  const installed = LLM_PROVIDERS.map((p) => p.id).filter((id) => providers.some((p) => p.id === id && p.installed));
  // Older settings may name a backend that no longer exists; they fall back to what's installed.
  return installed.find((id) => id === choice) ?? installed[0] ?? null;
}

export const llmEnabled = () => llmProvider() !== null;

/** One question to the LLM; rejects when it's off or the CLI fails. */
export async function llmCompose(system: string, user: string): Promise<string> {
  const provider = llmProvider();
  if (!provider) throw new Error("No LLM: install Claude Code or Codex, or turn it on in Settings");
  const { writer, writerModel } = useWorkspaces.getState().voiceConfig;
  // A model typed for the other CLI doesn't apply.
  const model = writer === provider ? (writerModel ?? "") : "";
  return invoke<string>("llm_compose", { req: { provider, model, system, user } });
}
