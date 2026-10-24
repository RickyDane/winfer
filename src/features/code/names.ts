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

/** Variants the friendly name drops but the picker must tell apart (fast twins are a toggle, not a row). */
export function modelTags(spec: string) {
  return /\[1m\]/i.test(tail(spec)) ? ["1M"] : [];
}

/** A backend's fast-tier twin of a model (`codex/gpt-6.1-sol-fast`), see `split_fast` in codex.rs. */
const FAST = "-fast";
export const isFast = (spec: string) => spec.endsWith(FAST) && tail(spec) !== FAST;
/** `codex/gpt-6.1-sol-fast` → `codex/gpt-6.1-sol`; other specs unchanged. */
export const baseSpec = (spec: string) => (isFast(spec) ? spec.slice(0, -FAST.length) : spec);
/** The spec of a model's fast twin, when the backend lists one. */
export const fastSpec = (spec: string, models: { spec: string }[]) => {
  const f = baseSpec(spec) + FAST;
  return models.some((m) => m.spec === f) ? f : null;
};

/** What a model is for, from its name and the backend's description of it (see `model_tier` in
 * tui/view.rs, which the TUI uses): small fast models, everyday ones, and the most capable. */
export type ModelTier = "fast" | "balanced" | "frontier";

const FAST_RE = /\b(haiku|mini|nano|flash|lite|small|spark|instant|fast|fastest|cheap)\b/;
const FRONTIER_RE = /\b(fable|opus|pro|max|ultra|most capable|frontier|flagship|complex|deep|hardest)\b/;

export function modelTier(spec: string, detail = ""): ModelTier {
  const name = baseSpec(spec).split("/").slice(1).join(" ").replace(/[-_.:[\]]/g, " ").toLowerCase();
  if (FAST_RE.test(name)) return "fast";
  if (FRONTIER_RE.test(name)) return "frontier";
  const d = detail.toLowerCase();
  if (FAST_RE.test(d)) return "fast";
  if (FRONTIER_RE.test(d)) return "frontier";
  return "balanced";
}
