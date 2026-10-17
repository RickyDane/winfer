import { invoke } from "@tauri-apps/api/core";

export type ProviderId = string;

/** Mirrors `ProviderInfo` in src-tauri/src/providers.rs. */
export interface Provider {
  id: ProviderId;
  name: string;
  command: string | null;
  args: string[];
  resumeArgs: string[];
  /** Resumes one session by id (`{session}` is replaced), when the CLI supports it. */
  resumeSessionArgs?: string[];
  aliases: string[];
  /** Reports status through hooks; otherwise status is inferred from output. */
  statusHooks: boolean;
  /** Accepts an initial prompt as an argument (otherwise it's typed in after start). */
  promptArg: boolean;
  /** Keys that accept a pending permission prompt (default Enter). */
  approveInput: string | null;
  /** Launch flag that selects the model, if the CLI has one. */
  modelArg: string | null;
  /** Typed into a running session to switch models (`{model}` is replaced), if supported. */
  modelCommand: string | null;
  /** Launch flags that skip permission prompts (offered as "auto-approve" in settings). */
  autoApproveArgs?: string[];
  installed: boolean;
}

export interface ModelInfo {
  id: string;
  label: string;
}

export const fetchProviders = () => invoke<Provider[]>("providers_list");
export const fetchModels = (id: ProviderId) => invoke<ModelInfo[]>("providers_models", { id });

/** Pane icon color per provider; unknown providers fall back to the muted foreground. */
export const PROVIDER_COLORS: Record<string, string> = {
  claude: "var(--color-claude)",
  codex: "var(--color-fg)",
  agy: "#4f8cff",
  grok: "var(--color-fg-muted)",
  opencode: "#f5b544",
  cursor: "#a0a0ff",
  pi: "#7ee787",
};
