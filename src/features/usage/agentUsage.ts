/**
 * Usage tools for agents (`usage_status` and `wait_for_usage_reset`, see src-tauri/src/mcp.rs):
 * an agent can look up how much of its plan's limits is left, and pause until they reset.
 * winfer then wakes it by typing a message into its pane. Pending wake-ups survive restarts.
 */
import { invoke } from "@tauri-apps/api/core";
import { pasteText, sendText } from "@/features/terminal/xtermRegistry";
import { logActivity } from "@/stores/activity";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { duration, leftNow, refreshCodex, runsOutAt, tightest, useUsage, usedNow, type UsageWindow } from "./usage";

/** OpenCode's model providers whose account sign-in shares the limits of a plan winfer tracks. */
const PLAN_OF: Record<string, string> = { openai: "codex", anthropic: "claude" };

/**
 * Whose limits a pane's agent uses: its own, or for OpenCode those of the plan it's signed in
 * to for the model it runs (ChatGPT for OpenAI models, the same limits as Codex). `note` says so.
 */
export async function limitsOf(pane: Pane): Promise<{ provider: string; note: string | null }> {
  if (pane.provider !== "opencode") return { provider: pane.provider, note: null };
  const model = await invoke<{ provider: string; model: string; plan: boolean } | null>("usage_opencode_model", { cwd: pane.cwd }).catch(() => null);
  const plan = model?.plan ? PLAN_OF[model.provider] : undefined;
  if (!model || !plan) return { provider: "opencode", note: model ? `OpenCode is using ${model.provider}/${model.model}${model.plan ? "" : " with an API key"}.` : null };
  const name = providerName(useWorkspaces.getState().providers, plan);
  return { provider: plan, note: `OpenCode is using ${model.provider}/${model.model} through your ${plan === "codex" ? "ChatGPT" : "Claude"} sign-in, so ${name}'s plan limits apply.` };
}

/** Fresh numbers where they can be asked for (Codex's are live; Claude's come with each reply). */
export async function refreshLimits(provider: string) {
  if (provider === "codex") await refreshCodex();
}

const round = (n: number) => Math.round(n * 10) / 10;

/** "2026-10-15 14:00 (in 1h 48m)": local time, which is what the agent's machine shows too. */
function at(ms: number, now: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  const local = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return ms > now ? `${local} (in ${duration(ms - now)})` : `${local} (passed)`;
}

/** What `usage_status` answers for an agent. */
export function usageReport(provider: string, now = Date.now()): string {
  const name = providerName(useWorkspaces.getState().providers, provider);
  const { snapshots, opencode } = useUsage.getState();
  const snapshot = snapshots[provider];
  if (!snapshot?.windows.length) {
    if (provider === "opencode") {
      const spend = opencode ? ` Today so far: ${opencode.tokens} tokens, $${opencode.cost.toFixed(2)}.` : "";
      return `OpenCode has no plan usage limits of its own, and the model it uses now isn't signed in with a plan winfer tracks (ChatGPT or Claude), so there is nothing to run out of in winfer's view.${spend}`;
    }
    if (provider === "claude" || provider === "codex") {
      return `winfer has no usage numbers for ${name} yet. They are reported after the agent's first answer in winfer; ask again after your next reply. (Sessions signed in with an API key have no plan limits.)`;
    }
    return `winfer can't read usage limits for ${name}.`;
  }
  const age = now - snapshot.at;
  const lines = [`${name} plan usage (measured ${age < 60_000 ? "just now" : `${duration(age)} ago`}, current time ${at(now, now).split(" (")[0]}):`];
  for (const w of snapshot.windows) {
    const reset = w.resetsAt !== null && w.resetsAt <= now;
    const parts = [`- ${w.label}: ${round(usedNow(w, now))}% used, ${round(leftNow(w, now))}% left`];
    if (reset) parts.push("(this window has reset since it was measured)");
    else if (w.resetsAt !== null) parts.push(`resets ${at(w.resetsAt, now)}`);
    const out = runsOutAt(provider, w, undefined, now);
    if (out !== null) parts.push(`at the current pace it runs out ${at(out, now)}, before it resets`);
    lines.push(parts.join(", "));
  }
  const tight = tightest(snapshot, now)!;
  lines.push(`Least left: ${tight.label}, ${round(leftNow(tight, now))}%.`);
  if (age > 30 * 60_000) lines.push("These numbers may be out of date: no agent has reported usage for a while.");
  return lines.join("\n");
}

// Waking agents after a reset ------------------------------------------------------------------

export interface Wake {
  paneId: string;
  /** Unix ms. */
  at: number;
  message: string;
}

const STORAGE = "winfer.usage.wakes";
/** Wait this long past the reset time, so the provider has surely reset the window. */
const MARGIN = 60_000;
export const DEFAULT_WAKE_MESSAGE = "Your usage limit has reset. Call usage_status to check how much is left, then continue where you left off.";

function loadWakes(): Wake[] {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE) ?? "[]");
    return Array.isArray(saved) ? saved : [];
  } catch {
    return [];
  }
}

let wakes: Wake[] = loadWakes();

function saveWakes() {
  try {
    localStorage.setItem(STORAGE, JSON.stringify(wakes));
  } catch {
    // Not kept across restarts then.
  }
}

export const pendingWake = (paneId: string) => wakes.find((w) => w.paneId === paneId) ?? null;

/**
 * Arranges for winfer to wake a pane when its limits reset: those with less than `below`
 * percent left (if none is that low, the one with least left), or after `minutes` when given.
 * Replaces the pane's earlier wake-up. Returns what to tell the agent.
 */
export function scheduleWake(paneId: string, provider: string, opts: { below?: number; minutes?: number; message?: string }, now = Date.now()): string {
  const message = opts.message?.trim() || DEFAULT_WAKE_MESSAGE;
  let due: number;
  let reason: string;
  if (opts.minutes !== undefined) {
    const minutes = Number(opts.minutes);
    if (!Number.isFinite(minutes) || minutes <= 0) throw new Error("minutes must be a positive number");
    due = now + minutes * 60_000;
    reason = `after ${duration(minutes * 60_000)}`;
  } else {
    const snapshot = useUsage.getState().snapshots[provider];
    const name = providerName(useWorkspaces.getState().providers, provider);
    const pending = (snapshot?.windows ?? []).filter((w): w is UsageWindow & { resetsAt: number } => w.resetsAt !== null && w.resetsAt > now);
    if (!pending.length) {
      throw new Error(`winfer doesn't know when ${name}'s limits reset (no usage numbers yet, or they have all reset already). Call usage_status, or pass minutes to wait a fixed time.`);
    }
    const below = opts.below === undefined ? 20 : Number(opts.below);
    const low = pending.filter((w) => leftNow(w, now) < below);
    const waitFor = low.length ? low : [pending.reduce((a, b) => (leftNow(b, now) < leftNow(a, now) ? b : a))];
    const last = waitFor.reduce((a, b) => (b.resetsAt > a.resetsAt ? b : a));
    due = last.resetsAt + MARGIN;
    reason = `when the ${waitFor.map((w) => w.label.toLowerCase()).join(" and ")} limit${waitFor.length > 1 ? "s reset" : " resets"}`;
  }
  wakes = [...wakes.filter((w) => w.paneId !== paneId), { paneId, at: due, message }];
  saveWakes();
  const ws = useWorkspaces.getState().workspaces.find((w) => w.panes.some((p) => p.id === paneId));
  if (ws) logActivity(ws.id, `${providerName(useWorkspaces.getState().providers, provider)} paused until ${at(due, now)}`, provider);
  return `Paused. winfer will type this into your pane ${reason}, at ${at(due, now)}: “${message}”\nStop working now: finish your reply without starting anything new, and end your turn. Don't poll or sleep while waiting.`;
}

export function cancelWake(paneId: string): boolean {
  const before = wakes.length;
  wakes = wakes.filter((w) => w.paneId !== paneId);
  saveWakes();
  return wakes.length !== before;
}

/** Sends the wake-ups that are due; drops those of panes that are gone. */
export function deliverWakes(now = Date.now()) {
  const panes = new Map(useWorkspaces.getState().workspaces.flatMap((w) => w.panes.map((p) => [p.id, w.id] as const)));
  const due = wakes.filter((w) => w.at <= now || !panes.has(w.paneId));
  if (!due.length) return;
  wakes = wakes.filter((w) => !due.includes(w));
  saveWakes();
  for (const wake of due) {
    const workspaceId = panes.get(wake.paneId);
    if (!workspaceId) continue;
    if (wake.message.includes("\n")) pasteText(wake.paneId, wake.message);
    else sendText(wake.paneId, wake.message);
    sendText(wake.paneId, "\r");
    logActivity(workspaceId, `Woke an agent: its usage limit has reset`);
  }
}

let started = false;

export function startWakes() {
  if (started) return;
  started = true;
  // Checked often rather than with one long timer, which a sleeping laptop would delay.
  window.setTimeout(() => deliverWakes(), 5000);
  window.setInterval(() => deliverWakes(), 30_000);
}
