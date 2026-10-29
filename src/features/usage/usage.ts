/**
 * How much of their plan's usage limits the agents have left (see src-tauri/src/usage.rs):
 * Claude Code is asked live (status line as fallback), Codex is asked live from ChatGPT (session logs
 * as fallback), OpenCode reports today's tokens and cost. Kept across restarts, with samples for a run-out projection.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isPermissionGranted, sendNotification } from "@tauri-apps/plugin-notification";
import { create } from "zustand";
import { toast } from "@/stores/toasts";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { accountLabel, accountsOf, defaultAccountId, hasAlternatives, loadAccounts, providerOfAccount, startAccount, useAccounts, usageKey, type Account } from "@/features/accounts/accounts";
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
  /** Codex: rate limit resets the account can redeem. */
  resetCredits?: number;
}

/** A Codex rate limit reset waiting to be redeemed. */
export interface ResetCredit {
  id: string;
  title: string | null;
  description: string | null;
  /** Unix ms. */
  grantedAt: number | null;
  expiresAt: number | null;
}

/** How full a pane's context window is. */
export interface Context {
  usedPercent: number;
  tokens: number;
  window: number;
  at: number;
  /** Where the conversation is kept (transcript, session log or session id), for the breakdown. */
  source?: string;
  /** Whose format `source` is in when it isn't the pane's agent's: winfer code's backend (claude, codex, chatgpt). */
  provider?: string;
}

/** One part of what fills a context window (estimated, see src-tauri/src/context_breakdown.rs). */
export interface ContextPart {
  kind: string;
  label: string;
  tokens: number;
  /** What it's made of, biggest first: for tool results, the tools that returned them. */
  sources?: { label: string; tokens: number }[];
}

/** What a pane's context is made of; null when the agent's session can't be read. */
export async function contextBreakdown(provider: string, context: Context): Promise<ContextPart[] | null> {
  if (!context.source) return null;
  return invoke<ContextPart[] | null>("usage_context_breakdown", { provider: context.provider ?? provider, source: context.source, tokens: context.tokens }).catch(() => null);
}

export interface Spend {
  tokens: number;
  cost: number;
  sessions: number;
}

/** What a pane's agent spent this session, as it reports it (winfer code). */
export interface PaneSpend {
  /** At API prices; null when the backend doesn't say (e.g. billed to a ChatGPT plan). */
  cost: number | null;
  input: number;
  output: number;
  cached: number;
  /** Output tokens of the running (or last) turn, partly estimated while it streams; harness panes only. */
  turn?: number;
  at: number;
}

interface UsageState {
  refreshing: Record<string, boolean>;
  errors: Record<string, string | undefined>;
  snapshots: Record<string, Snapshot>;
  opencode: Spend | null;
  /** Measurements per window ("codex:session:<resetsAt>") for the projection: [at, used%]. */
  samples: Record<string, [number, number][]>;
  /** Context use per pane. */
  context: Record<string, Context>;
  /** Codex session (thread) id per pane, from its turn-complete hook. */
  codexSessions: Record<string, string>;
  /** Spend per pane, for agents that report it themselves (not saved: it restarts with the agent). */
  paneSpend: Record<string, PaneSpend>;
}

const STORAGE = "winfer.usage";

function load(): Omit<UsageState, "opencode" | "paneSpend" | "refreshing" | "errors"> {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE) ?? "{}");
    return { snapshots: saved.snapshots ?? {}, samples: saved.samples ?? {}, context: saved.context ?? {}, codexSessions: saved.codexSessions ?? {} };
  } catch {
    return { snapshots: {}, samples: {}, context: {}, codexSessions: {} };
  }
}

export const useUsage = create<UsageState>(() => ({ ...load(), opencode: null, paneSpend: {}, refreshing: {}, errors: {} }));

useUsage.subscribe((s) =>
  localStorage.setItem(STORAGE, JSON.stringify({ snapshots: s.snapshots, samples: s.samples, context: s.context, codexSessions: s.codexSessions })),
);

export const windowKey = (provider: string, w: UsageWindow) => `${provider}:${w.kind}:${w.resetsAt ?? ""}`;

/**
 * Where the usage of the account new agents start on is kept (see accounts.ts `usageKey`): the
 * agent's id while that's its default sign-in.
 */
export function activeKey(provider: string, list = useAccounts.getState().list): string {
  return startAccount(provider, list) ?? provider;
}

/** Someone to tell when an account's plan runs out: (account id, reset time). */
export const exhaustedListeners = new Set<(accountId: string, resetAt: number | null) => void>();

/** Used percent now: a window whose reset time has passed starts over. */
export function usedNow(w: UsageWindow, now = Date.now()): number {
  return w.resetsAt !== null && w.resetsAt <= now ? 0 : w.usedPercent;
}

export const leftNow = (w: UsageWindow, now = Date.now()) => Math.max(0, Math.min(100, 100 - usedNow(w, now)));

/** Monthly credits only kick in once the plan's limits are used up. */
const isCredit = (w: UsageWindow) => w.kind === "month-credit";

/** The window that runs out first (least left): what the ring shows. Credits count only once
 *  every plan limit is out, since until then they aren't what's holding the agent back. */
export function tightest(snapshot: Snapshot | undefined, now = Date.now()): UsageWindow | null {
  if (!snapshot?.windows.length) return null;
  const least = (ws: UsageWindow[]) => ws.reduce((a, b) => (leftNow(b, now) < leftNow(a, now) ? b : a));
  const plan = snapshot.windows.filter((w) => !isCredit(w));
  if (!plan.length) return least(snapshot.windows);
  const tightPlan = least(plan);
  if (leftNow(tightPlan, now) > 0) return tightPlan;
  const credits = snapshot.windows.filter(isCredit);
  return credits.length ? credits.reduce((a, b) => (leftNow(b, now) > leftNow(a, now) ? b : a)) : tightPlan;
}

// Pooled accounts ---------------------------------------------------------------------------------

/** One of an agent's accounts as it counts toward the agent's pooled limits. */
export interface PoolMember {
  id: string;
  /** Where its usage is kept (see accounts.ts `usageKey`). */
  key: string;
  label: string;
  snapshot: Snapshot | undefined;
  /** Enabled and signed in: failover can move work to it, so its room counts. */
  counted: boolean;
}

/** An agent's accounts in failover order; just the default sign-in when there are no others. */
export function poolMembers(provider: string, snapshots: Record<string, Snapshot>, list: Account[]): PoolMember[] {
  const accounts = accountsOf(provider, list);
  if (accounts.length < 2) return [{ id: defaultAccountId(provider), key: provider, label: accountLabel(defaultAccountId(provider), list), snapshot: snapshots[provider], counted: true }];
  return accounts.map((a) => ({ id: a.id, key: usageKey(a.id), label: a.label, snapshot: snapshots[usageKey(a.id)], counted: a.enabled && a.signedIn }));
}

/**
 * An agent's limits over all its counted accounts: each account is 100%, so a window's "left" is
 * the average of what each account has left. It resets (partly) when the first account's does,
 * and is as old as the oldest reading in it.
 */
export function pooled(provider: string, snapshots: Record<string, Snapshot>, list: Account[], now = Date.now()): Snapshot | undefined {
  const members = poolMembers(provider, snapshots, list).filter((m) => m.counted && m.snapshot?.windows.length);
  if (members.length === 0) return snapshots[provider]?.windows.length ? snapshots[provider] : undefined;
  if (members.length === 1) return { ...members[0].snapshot!, provider };
  const kinds = [...new Set(members.flatMap((m) => m.snapshot!.windows.map((w) => w.kind)))];
  const windows = kinds.map((kind) => {
    const ws = members.flatMap((m) => m.snapshot!.windows.filter((w) => w.kind === kind));
    const left = ws.reduce((sum, w) => sum + leftNow(w, now), 0) / ws.length;
    const resets = ws.map((w) => w.resetsAt).filter((t): t is number => t !== null && t > now);
    return { kind, label: ws[0].label, usedPercent: 100 - left, resetsAt: resets.length ? Math.min(...resets) : null };
  });
  return { provider, windows, at: Math.min(...members.map((m) => m.snapshot!.at)) };
}

/** Readings of a pooled window over time ([at, used%]): at each reading, the accounts' average. */
export function pooledSamples(provider: string, kind: string, snapshots: Record<string, Snapshot>, list: Account[], samples = useUsage.getState().samples, now = Date.now()): [number, number][] {
  const members = poolMembers(provider, snapshots, list).filter((m) => m.counted && m.snapshot?.windows.some((w) => w.kind === kind));
  const series = members.map((m) => {
    const w = m.snapshot!.windows.find((x) => x.kind === kind)!;
    const live = w.resetsAt !== null && w.resetsAt > now;
    return { points: live ? (samples[windowKey(m.key, w)] ?? []) : [], current: usedNow(w, now) };
  });
  if (members.length < 2) return series[0]?.points ?? [];
  const times = [...new Set(series.flatMap((s) => s.points.map(([t]) => t)))].sort((a, b) => a - b);
  // Each account at its last reading by then; before its first one, as it was first seen.
  const at = (s: (typeof series)[number], t: number) => [...s.points].reverse().find(([pt]) => pt <= t)?.[1] ?? s.points[0]?.[1] ?? s.current;
  return times.map((t) => [t, series.reduce((sum, s) => sum + at(s, t), 0) / series.length]);
}

/** When a pooled window runs out at the combined pace of its accounts, if before any of them resets. */
export function pooledRunsOutAt(provider: string, kind: string, snapshots: Record<string, Snapshot>, list: Account[], samples = useUsage.getState().samples, now = Date.now()): number | null {
  const members = poolMembers(provider, snapshots, list).filter((m) => m.counted && m.snapshot?.windows.some((w) => w.kind === kind));
  if (members.length < 2) {
    const m = members[0];
    const w = m?.snapshot!.windows.find((x) => x.kind === kind);
    return m && w ? runsOutAt(m.key, w, samples, now) : null;
  }
  let left = 0;
  let perMs = 0;
  let firstReset = Infinity;
  for (const m of members) {
    const w = m.snapshot!.windows.find((x) => x.kind === kind)!;
    left += leftNow(w, now);
    if (w.resetsAt !== null && w.resetsAt > now) firstReset = Math.min(firstReset, w.resetsAt);
    const points = samples[windowKey(m.key, w)] ?? [];
    if (points.length < 2) continue;
    const [t0, u0] = points[0];
    const [t1, u1] = points[points.length - 1];
    if (t1 - t0 >= 15 * 60_000 && u1 > u0) perMs += (u1 - u0) / (t1 - t0);
  }
  if (perMs <= 0) return null;
  const at = now + left / perMs;
  return at < firstReset ? at : null;
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

async function warn(key: string, w: UsageWindow, left: number) {
  const provider = providerOfAccount(key);
  const account = key === provider ? `${provider}:default` : key;
  const many = accountsOf(provider).length > 1;
  const name = providerName(useWorkspaces.getState().providers, provider) + (many ? ` (${accountLabel(account)})` : "");
  if (left <= 0) exhaustedListeners.forEach((l) => l(account, w.resetsAt));
  // Out, with another account to move to: the failover says what happens instead.
  if (left <= 0 && many && hasAlternatives(provider)) return;
  const text = left <= 0 ? `${name}: ${w.label.toLowerCase()} limit reached` : `${name}: ${Math.round(left)}% of the ${w.label.toLowerCase()} limit left`;
  const detail = w.resetsAt ? `Resets ${when(w.resetsAt)}` : undefined;
  // At the limit: offer to carry the work over to an agent that still has room.
  const target = left <= 0 ? handoffTarget(provider) : null;
  const pane = target ? paneToHandOff(provider) : null;
  // A Codex reset puts the limits back to full: offer it before handing work elsewhere.
  const resets = left <= 0 && provider === "codex" ? (useUsage.getState().snapshots.codex?.resetCredits ?? 0) : 0;
  const action = resets > 0
    ? { label: "Use a reset", run: () => void redeemCodexReset().then(toastResetOutcome, (e: unknown) => toast(String(e), "error")) }
    : target && pane ? { label: `Continue in ${providerName(useWorkspaces.getState().providers, target)}`, run: () => continueIn(pane, target) } : undefined;
  toast(text, left <= 5 ? "error" : "info", detail, action);
  if (!document.hasFocus() && (await isPermissionGranted().catch(() => false))) sendNotification({ title: text, body: detail });
}

function record(snapshot: Snapshot) {
  // `provider` is the usage key: an account other than the default is kept under its own id.
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
      const resets = Number(key.split(":").at(-1));
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

/** The first read after launch replaces whatever was kept: Codex is the source of truth. */
let codexRead = false;

const pending = new Map<string, Promise<void>>();
let claudeLiveAt = 0;

/** Share overlapping automatic/manual requests, and keep failures separate from cached readings. */
function refresh(provider: string, read: () => Promise<void>): Promise<void> {
  const active = pending.get(provider);
  if (active) return active;
  useUsage.setState((s) => ({ refreshing: { ...s.refreshing, [provider]: true } }));
  const request = Promise.resolve().then(read).then(() => {
    useUsage.setState((s) => ({ errors: { ...s.errors, [provider]: undefined } }));
  }).catch((error: unknown) => {
    useUsage.setState((s) => ({ errors: { ...s.errors, [provider]: String(error) } }));
  }).finally(() => {
    pending.delete(provider);
    useUsage.setState((s) => ({ refreshing: { ...s.refreshing, [provider]: false } }));
  });
  pending.set(provider, request);
  return request;
}

export function refreshCodex() {
  return refresh("codex", async () => {
    const snapshot = await invoke<Snapshot | null>("usage_codex");
    if (!snapshot) throw new Error("Couldn't refresh Codex limits. Check your Codex sign-in.");
    const current = useUsage.getState().snapshots.codex;
    if (!codexRead || !current || snapshot.at >= current.at) record(snapshot);
    codexRead = true;
  });
}

/** The Codex resets the account can redeem now. */
export const listCodexResets = () => invoke<ResetCredit[]>("usage_codex_resets");

export type ResetCode = "reset" | "nothing_to_reset" | "no_credit" | "already_redeemed" | (string & {});

/** Redeems a Codex reset (the given one, else the server's pick), then rereads the limits. */
export async function redeemCodexReset(creditId?: string): Promise<{ code: ResetCode; windowsReset: number }> {
  try {
    return await invoke<{ code: ResetCode; windowsReset: number }>("usage_codex_redeem_reset", { creditId: creditId ?? null });
  } finally {
    await refreshCodex();
  }
}

export function resetOutcomeText(code: ResetCode): string {
  switch (code) {
    case "reset": return "Codex limits reset — full again.";
    case "nothing_to_reset": return "Nothing to reset: Codex's limits aren't used. The reset was kept.";
    case "no_credit": return "No Codex reset available.";
    case "already_redeemed": return "That reset was already used.";
    default: return `Codex answered “${code}”.`;
  }
}

export function toastResetOutcome({ code }: { code: ResetCode }) {
  toast(resetOutcomeText(code), code === "reset" ? "success" : "info");
}

export function refreshClaude() {
  return refresh("claude", async () => {
    const snapshot = await invoke<Snapshot>("usage_claude");
    if (!snapshot?.windows.length) throw new Error("Claude returned no plan limits.");
    const current = useUsage.getState().snapshots.claude;
    if (!current || snapshot.at >= current.at) record(snapshot);
    claudeLiveAt = snapshot.at;
  });
}

/** Reads the limits of an account other than the default one (see accounts.ts). */
export function refreshAccount(id: string) {
  const key = usageKey(id);
  const provider = providerOfAccount(id);
  if (key === provider) return provider === "codex" ? refreshCodex() : refreshClaude();
  return refresh(key, async () => {
    const snapshot = provider === "codex"
      ? await invoke<Snapshot | null>("usage_codex", { account: id })
      : await invoke<Snapshot>("usage_claude", { account: id });
    if (!snapshot?.windows.length) throw new Error(`Couldn't refresh the limits of ${accountLabel(id)}. Check its sign-in.`);
    record({ ...snapshot, provider: key });
  });
}

/** The other signed-in accounts' limits (Claude's at most every 5 minutes, like the default). */
const accountPolledAt = new Map<string, number>();
export function refreshOtherAccounts(force = false) {
  return Promise.all(useAccounts.getState().list.filter((a) => a.signedIn && a.enabled && usageKey(a.id) !== a.provider).map((a) => {
    const due = a.provider === "codex" || Date.now() - (accountPolledAt.get(a.id) ?? 0) >= 5 * 60_000;
    if (!force && !due) return undefined;
    accountPolledAt.set(a.id, Date.now());
    return refreshAccount(a.id);
  }));
}

export async function refreshUsage() {
  const { providers } = useWorkspaces.getState();
  const known = useUsage.getState().snapshots;
  // Before the provider list loads, fall back to agents that reported before.
  const installed = (id: string) => (providers.length ? providers.some((p) => p.id === id && p.installed) : !!known[id]);
  await Promise.all([
    installed("codex") ? refreshCodex() : undefined,
    installed("claude") ? refreshClaude() : undefined,
    installed("opencode") ? refreshOpencode() : undefined,
    refreshOtherAccounts(true),
  ]);
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

export function refreshOpencode() {
  return refresh("opencode", async () => {
    const spend = await invoke<Spend | null>("usage_opencode");
    if (!spend) throw new Error("Couldn't refresh OpenCode usage.");
    useUsage.setState({ opencode: spend });
  });
}

let started = false;

export function startUsageTracking() {
  if (started) return;
  started = true;
  void listen<{ provider: string; paneId: string; report: { windows?: UsageWindow[]; context?: Context | null; spend?: PaneSpend | null } }>("usage://report", ({ payload }) => {
    // Status-line limits may be cached by Claude Code. Prefer a recent account-wide read.
    // A pane on another account reports that account's limits.
    const pane = useWorkspaces.getState().workspaces.flatMap((w) => w.panes).find((p) => p.id === payload.paneId);
    const key = pane?.accountId && providerOfAccount(pane.accountId) === payload.provider ? usageKey(pane.accountId) : payload.provider;
    if (payload.report.windows?.length && !(key === "claude" && Date.now() - claudeLiveAt < 5 * 60_000)) {
      record({ provider: key, windows: payload.report.windows, at: Date.now() });
    }
    if (payload.report.context) setContext(payload.paneId, payload.report.context);
    const spend = payload.report.spend;
    if (spend) useUsage.setState((s) => ({ paneSpend: { ...s.paneSpend, [payload.paneId]: spend } }));
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
  const installed = (id: string) => {
    const { providers } = useWorkspaces.getState();
    // Before the provider list loads, fall back to agents that reported before.
    return providers.length ? providers.some((p) => p.id === id && p.installed) : !!useUsage.getState().snapshots[id];
  };
  // Just a look at a few log files (nothing if Codex was never used), so no need to wait for
  // the provider list.
  let claudePolledAt = 0;
  const poll = () => {
    void loadAccounts().then(() => refreshOtherAccounts());
    void refreshCodex();
    if (installed("claude") && Date.now() - Math.max(claudePolledAt, claudeLiveAt) >= 5 * 60_000) {
      claudePolledAt = Date.now();
      void refreshClaude();
    }
  };
  poll();
  window.setTimeout(poll, 3000);
  window.setInterval(poll, 60_000);
  window.addEventListener("focus", poll);
  const pollOpencode = () => installed("opencode") && void refreshOpencode();
  window.setTimeout(pollOpencode, 3000);
  window.setInterval(pollOpencode, 5 * 60_000);
}
