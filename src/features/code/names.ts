/** Model names as the pane headers show them, with the version a backend resolved an alias to. */
import { displayModelName } from "@/lib/modelName";
import { useCodeWeb } from "./styles";

/** winfer code's backends: their spec prefix, name and logo (see `harness::backend::Kind`). */
export const BACKENDS: { id: string; label: string; logo: string }[] = [
  { id: "chatgpt", label: "ChatGPT", logo: "openai" },
  { id: "claude", label: "Claude Code", logo: "claude" },
  { id: "codex", label: "Codex", logo: "codex" },
  { id: "agy", label: "Antigravity", logo: "agy" },
  { id: "cursor", label: "Cursor", logo: "cursor" },
  { id: "opencode", label: "OpenCode", logo: "opencode" },
  { id: "pi", label: "Pi", logo: "pi" },
];

/** The backend a spec runs on (`opencode/anthropic/claude-sonnet` → opencode). */
export const backendOf = (spec: string) => BACKENDS.find((b) => spec.startsWith(b.id + "/") || spec === b.id);

/** A `backend/model` spec, as opposed to a model id that contains a slash (`anthropic/claude-…`). */
export const isSpec = (s: string) => !!backendOf(s);

const tail = (spec: string) => (backendOf(spec) ? spec.slice(spec.indexOf("/") + 1) : spec);

/** Remembers what an alias (claude/opus) resolved to, so the picker shows versions for every one it has seen. */
export function rememberResolved(spec: string, actual: string) {
  if (!actual || actual === tail(spec)) return;
  const { resolvedModels, set } = useCodeWeb.getState();
  if (resolvedModels[spec] !== actual) set({ resolvedModels: { ...resolvedModels, [spec]: actual } });
}

/** `claude/opus` (resolved to claude-opus-5-5) → "Opus 5.5"; `codex/gpt-6.1-sol-fast` → "GPT 6.1 Sol". */
export function modelName(spec: string, resolved: Record<string, string>, actual?: string | null) {
  return displayModelName(actual || resolved[spec] || tail(spec));
}

/** Variants the friendly name drops but the picker must tell apart. */
export function modelTags(spec: string, label = "") {
  const t = tail(spec).toLowerCase();
  const tags: string[] = [];
  if (t.endsWith("-fast") || /\(fast\)/i.test(label)) tags.push("fast");
  if (/\[1m\]/.test(t)) tags.push("1M");
  return tags;
}
