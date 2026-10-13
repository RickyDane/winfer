import { invoke } from "@tauri-apps/api/core";

export type ProviderId = string;

/** Mirrors `ProviderInfo` in src-tauri/src/providers.rs. */
export interface Provider {
  id: ProviderId;
  name: string;
  command: string | null;
  args: string[];
  resumeArgs: string[];
  aliases: string[];
  /** Reports status through hooks; otherwise status is inferred from output. */
  statusHooks: boolean;
  /** Accepts an initial prompt as an argument (otherwise it's typed in after start). */
  promptArg: boolean;
  /** Keys that accept a pending permission prompt (default Enter). */
  approveInput: string | null;
  installed: boolean;
}

export const fetchProviders = () => invoke<Provider[]>("providers_list");

/** Pane icon color per provider; unknown providers fall back to the muted foreground. */
export const PROVIDER_COLORS: Record<string, string> = {
  claude: "var(--color-claude)",
  codex: "#e6e6ea",
  agy: "#4f8cff",
  grok: "#c9c9d1",
  opencode: "#f5b544",
  cursor: "#a0a0ff",
};
