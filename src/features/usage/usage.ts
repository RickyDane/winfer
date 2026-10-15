/**
 * How much of their plan's usage limits the agents have left (see src-tauri/src/usage.rs):
 * Claude Code reports through its status line, Codex is read from its session logs, OpenCode
 * reports today's tokens and cost. Kept across restarts, with samples for a run-out projection.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isPermissionGranted, sendNotification } from "@tauri-apps/plugin-notification";
import { create } from "zustand";
import { toast } from "@/stores/toasts";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { continueIn, handoffTarget, paneToHandOff } from "./handoff";

export interface UsageWindow {
  /** "session" (5-hour), "week", … */
  kind: string;
  label: string;
  usedPercent: number;
  /** Unix ms. */
  resetsAt: number | null;
}

export interface Snapshot {
  provider: string;
  windows: UsageWindow[];
  /** When it was measured (unix ms). */
  at: number;
}

/** How full a pane's context window is. */
export interface Context {
  usedPercent: number;
  tokens: number;
  window: number;
  at: number;
}

export interface Spend {
  tokens: number;
  cost: number;
  sessions: number;
}

interface UsageState {
  snapshots: Record<string, Snapshot>;
  opencode: Spend | null;
  /** Measurements per window ("codex:session:<resetsAt>") for the projection: [at, used%]. */
  samples: Record<string, [number, number][]>;
  /** Context use per pane. */
  context: Record<string, Context>;
  /** Codex session (thread) id per pane, from its turn-complete hook. */
  codexSessions: Record<string, string>;
}

const STORAGE = "winfer.usage";

function load(): Omit<UsageState, "opencode"> {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE) ?? "{}");
    return { snapshots: saved.snapshots ?? {}, samples: saved.samples ?? {}, context: saved.context ?? {}, codexSessions: saved.codexSessions ?? {} };
  } catch {
    return { snapshots: {}, samples: {}, context: {}, codexSessions: {} };
  }
}

export const useUsage = create<UsageState>(() => ({ ...load(), opencode: null }));

useUsage.subscribe((s) =>
  localStorage.setItem(STORAGE, JSON.stringify({ snapshots: s.snapshots, samples: s.samples, context: s.context, codexSessions: s.codexSessions })),
);

const windowKey = (provider: string, w: UsageWindow) => `${provider}:${w.kind}:${w.resetsAt ?? ""}`;

/** Used percent now: a window whose reset time has passed starts over. */
export function usedNow(w: UsageWindow, now = Date.now()): number {
  return w.resetsAt !== null && w.resetsAt <= now ? 0 : w.usedPercent;
}

export const leftNow = (w: UsageWindow, now = Date.now()) => Math.max(0, Math.min(100, 100 - usedNow(w, now)));

/** The window that runs out first (least left): what the ring shows. */
export function tightest(snapshot: Snapshot | undefined, now = Date.now()): UsageWindow | null {
  if (!snapshot?.windows.length) return null;
  return snapshot.windows.reduce((a, b) => (leftNow(b, now) < leftNow(a, now) ? b : a));
}

/** When the window runs out at the pace measured in it, if that's before it resets. */
export function runsOutAt(provider: string, w: UsageWindow, samples = useUsage.getState().samples, now = Date.now()): number | null {
  if (w.resetsAt === null || w.resetsAt <= now) return null;
  const points = samples[windowKey(provider, w)] ?? [];
  if (points.length < 2) return null;
  const [t0, u0] = points[0];
  const [t1, u1] = points[points.length - 1];
  // Too little to go on: under 15 minutes of history or no usage in it.
  if (t1 - t0 < 15 * 60_000 || u1 <= u0) return null;
  const perMs = (u1 - u0) / (t1 - t0);
  const at = t1 + (100 - u1) / perMs;
  return at < w.resetsAt ? at : null;
}

/** Ring and bar colours by how much is left. */
export function levelColor(left: number): string {
  if (left < 10) return "var(--color-danger)";
  if (left < 30) return "var(--color-warning)";
  return "var(--color-success)";
}

/** "1h 48m", "3d 4h", "12m". */
export function duration(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;
  if (days > 0) return hours ? `${days}d ${hours}h` : `${days}d`;
  if (hours > 0) return mins ? `${hours}h ${mins}m` : `${hours}h`;
  return `${mins}m`;
}

/** "Thu 09:00" for times more than a day away, else "in 1h 48m". */
export function when(at: number, now = Date.now()): string {
  if (at - now < 24 * 3600_000) return `in ${duration(at - now)}`;
  return new Date(at).toLocaleString(undefined, { weekday: "short", hour: "2-digit", minute: "2-digit" });
}

// Thresholds -----------------------------------------------------------------------------------

const WARN_AT = [20, 5];
const warned = new Set<string>();

async function warn(provider: string, w: UsageWindow, left: number) {
  const name = providerName(useWorkspaces.getState().providers, provider);
  const text = left <= 0 ? `${name}: ${w.label.toLowerCase()} limit reached` : `${name}: ${Math.round(left)}% of the ${w.label.toLowerCase()} limit left`;
  const detail = w.resetsAt ? `Resets ${when(w.resetsAt)}` : undefined;
  // At the limit: offer to carry the work over to an agent that still has room.
  const target = left <= 0 ? handoffTarget(provider) : null;
  const pane = target ? paneToHandOff(provider) : null;
  const action = target && pane ? { label: `Continue in ${providerName(useWorkspaces.getState().providers, target)}`, run: () => continueIn(pane, target) } : undefined;
  toast(text, left <= 5 ? "error" : "info", detail, action);
  if (!document.hasFocus() && (await isPermissionGranted().catch(() => false))) sendNotification({ title: text, body: detail });
}

function record(snapshot: Snapshot) {
  const before = useUsage.getState().snapshots[snapshot.provider];
  useUsage.setState((s) => {
    const samples = { ...s.samples };
    for (const w of snapshot.windows) {
      const key = windowKey(snapshot.provider, w);
      const list = samples[key] ?? [];
      const last = list.at(-1);
      // Claude reports on every status line refresh: keep changes, and otherwise one per 5 minutes.
      if (!last || (last[0] !== snapshot.at && (last[1] !== w.usedPercent || snapshot.at - last[0] > 5 * 60_000))) {
        samples[key] = [...list, [snapshot.at, w.usedPercent] as [number, number]].slice(-200);
      }
    }
    // Drop samples of windows that have reset.
    const now = Date.now();
    for (const key of Object.keys(samples)) {
      const resets = Number(key.split(":")[2]);
      if (resets && resets < now) delete samples[key];
    }
    return { snapshots: { ...s.snapshots, [snapshot.provider]: snapshot }, samples };
  });
  // Warn once per window and threshold, only when crossing it (not on the very first reading).
  if (!before) return;
  for (const w of snapshot.windows) {
    const left = leftNow(w);
    const prev = before?.windows.find((b) => b.kind === w.kind);
    const prevLeft = prev ? leftNow(prev) : 100;
    const crossed = [...WARN_AT, 0].find((t) => left <= t && prevLeft > t);
    const key = `${windowKey(snapshot.provider, w)}:${crossed}`;
    if (crossed !== undefined && !warned.has(key)) {
      warned.add(key);
      void warn(snapshot.provider, w, left);
    }
  }
}

/** The first read after launch replaces whatever was kept: the logs are the source of truth. */
let codexRead = false;

export async function refreshCodex() {
  const snapshot = await invoke<Snapshot | null>("usage_codex").catch(() => null);
  const current = useUsage.getState().snapshots.codex;
  if (snapshot && (!codexRead || !current || snapshot.at >= current.at)) record(snapshot);
  if (snapshot) codexRead = true;
}

function setContext(paneId: string, context: Context) {
  useUsage.setState((s) => ({ context: { ...s.context, [paneId]: context } }));
}

/** Reads a Codex pane's context use from its session log. */
export async function refreshCodexContext(pane: Pane) {
  const session = useUsage.getState().codexSessions[pane.id] ?? null;
  const context = await invoke<Context | null>("usage_codex_context", { session, cwd: pane.cwd }).catch(() => null);
  if (context) setContext(pane.id, context);
}

/** Reads an OpenCode pane's context use from OpenCode's session database. */
export async function refreshOpencodeContext(pane: Pane) {
  const context = await invoke<Context | null>("usage_opencode_context", { cwd: pane.cwd }).catch(() => null);
  if (context) setContext(pane.id, context);
}

const panesOf = (provider: string) => useWorkspaces.getState().workspaces.flatMap((w) => w.panes).filter((p) => p.provider === provider);
const codexPanes = () => panesOf("codex");

/** Context colour: fine until it gets full enough for the agent to start compacting. */
export function contextColor(used: number): string {
  if (used >= 85) return "var(--color-danger)";
  if (used >= 60) return "var(--color-warning)";
  return "var(--color-accent)";
}

/** "84k", "1.2M". */
export function tokens(n: number): string {
  return n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}M` : n >= 1000 ? `${Math.round(n / 1000)}k` : String(n);
}

export async function refreshOpencode() {
  const spend = await invoke<Spend | null>("usage_opencode").catch(() => null);
  useUsage.setState({ opencode: spend });
}

let started = false;

export function startUsageTracking() {
  if (started) return;
  started = true;
  void listen<{ provider: string; paneId: string; report: { windows?: UsageWindow[]; context?: Context | null } }>("usage://report", ({ payload }) => {
    if (payload.report.windows?.length) record({ provider: payload.provider, windows: payload.report.windows, at: Date.now() });
    if (payload.report.context) setContext(payload.paneId, payload.report.context);
  });
  // Codex writes its numbers after each turn; a turn ending is the moment to look.
  void listen<{ provider: string; paneId: string; session?: string | null }>("agent://status", ({ payload }) => {
    if (payload.provider !== "codex") return;
    if (payload.session) useUsage.setState((s) => ({ codexSessions: { ...s.codexSessions, [payload.paneId]: payload.session! } }));
    window.setTimeout(() => {
      void refreshCodex();
      const pane = codexPanes().find((p) => p.id === payload.paneId);
      if (pane) void refreshCodexContext(pane);
    }, 1500);
  });
  // Codex also logs token counts during a turn: follow working panes, and read all at startup.
  window.setTimeout(() => codexPanes().forEach((p) => void refreshCodexContext(p)), 3000);
  // OpenCode reports no token counts: look at its sessions now and then (one small database query each).
  const pollOpencodeContext = () => panesOf("opencode").forEach((p) => void refreshOpencodeContext(p));
  window.setTimeout(pollOpencodeContext, 3000);
  window.setInterval(pollOpencodeContext, 20_000);
  window.setInterval(() => codexPanes().filter((p) => p.status === "working").forEach((p) => void refreshCodexContext(p)), 30_000);
  const installed = (id: string) => useWorkspaces.getState().providers.some((p) => p.id === id && p.installed);
  // Just a look at a few log files (nothing if Codex was never used), so no need to wait for
  // the provider list.
  const poll = () => void refreshCodex();
  poll();
  window.setInterval(poll, 60_000);
  window.addEventListener("focus", poll);
  const pollOpencode = () => installed("opencode") && void refreshOpencode();
  window.setTimeout(pollOpencode, 3000);
  window.setInterval(pollOpencode, 5 * 60_000);
}
