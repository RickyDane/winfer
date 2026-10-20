/**
 * The mobile companion's webview side (docs/mobile/protocol.md). Rust (src-tauri/src/mobile/)
 * keeps the paired phones, the relay connections and push; panes live here, so phone requests
 * arrive as `mobile://request` and answers go back with `mobile_send`.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { defaultProviderId, paneLabel, useWorkspaces, type AgentStatus, type Pane as WorkspacePane, type Workspace } from "@/stores/workspaces";
import { readTerminal, sendText, startDetached } from "@/features/terminal/xtermRegistry";
import { leaves } from "@/features/layout/tree";
import { execute } from "@/commands/bus";
import { logActivity } from "@/stores/activity";
import { describeRequest, type AgentRequest } from "@/features/status/request";
import { lastReply } from "@/features/status/replies";
import { useUsage } from "@/features/usage/usage";
import { answerApproval, approvalFor, useApprovals } from "@/features/plugins/approvals";

/** A pane as the phone sees it. */
export interface MobilePane {
  id: string;
  title: string;
  workspace: string;
  workspaceId: string;
  /** Put aside in the desktop's archive (still running). */
  archived: boolean;
  provider: string | null;
  status: AgentStatus;
  message: string | null;
  updatedAt: number;
  request: AgentRequest | null;
  reply: string | null;
}

export interface MobileMessage {
  id: string;
  role: "user" | "assistant" | "tool";
  text: string;
  tool?: string;
  ts: number | null;
}

/** A workspace as the phone sees it, in the desktop's sidebar order. */
export interface MobileWorkspace {
  id: string;
  name: string;
  /** The workspace open on the desktop. */
  active: boolean;
}

/** An agent the phone can start a session with. */
export interface MobileProvider {
  id: string;
  name: string;
}

type Request = { t: string; id?: string; paneId?: string; workspaceId?: string; provider?: string; text?: string; limit?: number; option?: number };

/** Each pane's latest status message (the hook's or notify_user's) and when its status changed. */
const messages = new Map<string, string | null>();
const changedAt = new Map<string, number>();
/** Needs-input panes' requests, read from the screen once the prompt is drawn. */
const requests = new Map<string, AgentRequest>();
/** When each pending plugin approval was asked. */
const approvalAt = new Map<number, number>();

/** Called by reportStatus with the message that comes with a status change. */
export function noteStatusMessage(paneId: string, message: string | undefined) {
  messages.set(paneId, message ?? null);
}

const allPanes = () => useWorkspaces.getState().workspaces.flatMap((w) => w.panes);

/** A workspace's panes as the desktop shows them: tiles in layout order, then the rest (archive). */
function ordered(ws: Workspace): WorkspacePane[] {
  const tiled = ws.layout ? leaves(ws.layout).flatMap((id) => ws.panes.find((p) => p.id === id) ?? []) : [];
  return [...tiled, ...ws.panes.filter((p) => !tiled.includes(p))];
}

/** Replies longer than this are cut in the pane list (the thread has them whole). */
const MAX_REPLY = 4000;

/** Everything the phone mirrors: every pane of every workspace, the workspaces, the agents. */
export function mirror() {
  const s = useWorkspaces.getState();
  return {
    panes: s.workspaces.flatMap(ordered).map(snapshot),
    workspaces: s.workspaces.map((w): MobileWorkspace => ({ id: w.id, name: w.name, active: w.id === s.activeWorkspaceId })),
    providers: s.providers.filter((p) => p.installed && p.id !== "shell").map((p): MobileProvider => ({ id: p.id, name: p.name })),
    defaultProvider: defaultProviderId(s),
  };
}

export function snapshot(pane: WorkspacePane): MobilePane {
  const s = useWorkspaces.getState();
  const base: MobilePane = {
    id: pane.id,
    title: paneLabel(pane, s.providers, null, s.paneTasks[pane.id]?.title),
    workspace: s.workspaces.find((w) => w.id === pane.workspaceId)?.name ?? "",
    workspaceId: pane.workspaceId,
    archived: pane.archived ?? false,
    provider: pane.provider === "shell" ? null : pane.provider,
    status: pane.status,
    message: messages.get(pane.id) ?? null,
    updatedAt: changedAt.get(pane.id) ?? Date.now(),
    request: pane.status === "needs-input" ? (requests.get(pane.id) ?? null) : null,
    reply: pane.status === "done" ? cutReply(lastReply(pane.id)) : null,
  };
  // A plugin asking for approval on this agent's behalf (its tool call waits on the answer).
  const approval = approvalFor(pane.id);
  if (!approval) return base;
  return {
    ...base,
    status: "needs-input",
    message: approval.title,
    updatedAt: approvalAt.get(approval.id) ?? Date.now(),
    request: { kind: "approval", text: `${approval.title}\n\n${approval.message}` },
    reply: null,
  };
}

const cutReply = (text: string | null) => (text && text.length > MAX_REPLY ? `${text.slice(0, MAX_REPLY)}…` : text);

const send = (device: string | null, msg: unknown) => invoke("mobile_send", { device, msg }).catch(() => {});

/** The phone's thread: the agent's session when winfer can read it, else the screen. */
async function thread(pane: WorkspacePane, limit: number): Promise<{ messages: MobileMessage[]; source: string }> {
  // winfer code keeps its own transcript whatever backend it runs on; its usage context names
  // the backend's, which may not exist (API backends) or lacks winfer's tool summaries.
  if (pane.provider === "winfer" && pane.sessionId) {
    const messages = await invoke<MobileMessage[] | null>("mobile_thread", { provider: "winfer", source: pane.sessionId, limit }).catch(() => null);
    if (messages?.length) return { messages, source: "winfer" };
  }
  const context = useUsage.getState().context[pane.id];
  const provider = context?.provider ?? pane.provider;
  if (pane.provider !== "winfer" && context?.source) {
    const messages = await invoke<MobileMessage[] | null>("mobile_thread", { provider, source: context.source, limit }).catch(() => null);
    if (messages?.length) return { messages, source: provider };
  }
  const screen = (readTerminal(pane.id, 120) ?? "").replace(/\s+$/, "");
  return { messages: screen ? [{ id: "screen", role: "assistant", text: "```\n" + screen + "\n```", ts: Date.now() }] : [], source: "screen" };
}

async function onRequest(device: string, req: Request) {
  const reply = (msg: Record<string, unknown>) => send(device, { ...msg, id: req.id });
  if (req.t === "panes.get") {
    return reply({ t: "panes", ...mirror() });
  }
  if (req.t === "pane.new") return reply(newPane(req));
  const pane = allPanes().find((p) => p.id === req.paneId);
  if (!pane) return reply({ t: req.t === "thread.get" ? "error" : "ack", ok: false, error: "That agent is gone." });
  const target = { id: pane.id };
  switch (req.t) {
    case "thread.get": {
      const { messages, source } = await thread(pane, Math.min(Math.max(req.limit ?? 200, 1), 500));
      return reply({ t: "thread", paneId: pane.id, messages, source });
    }
    case "send": {
      const text = req.text?.trim();
      if (!text) return reply({ t: "ack", ok: false, error: "Nothing to send." });
      execute({ type: "pane.sendText", target, text, submit: true });
      logActivity(pane.workspaceId, `Answered from your phone: ${text.slice(0, 80)}`, pane.provider);
      return reply({ t: "ack", ok: true });
    }
    case "choose": {
      // Typing an option's number picks it in Claude Code's and winfer code's pickers.
      const n = req.option;
      const options = requests.get(pane.id)?.options ?? [];
      if (pane.status !== "needs-input" || !Number.isInteger(n) || !options.some((o) => o.n === n) || n! > 9)
        return reply({ t: "ack", ok: false, error: "That choice is no longer on screen." });
      sendText(pane.id, String(n));
      const text = req.text?.trim();
      // "Type something" rows open a text field: type the answer into it.
      if (text) setTimeout(() => execute({ type: "pane.sendText", target, text, submit: true }), 250);
      logActivity(pane.workspaceId, `Chose "${options.find((o) => o.n === n)!.label.slice(0, 60)}" from your phone`, pane.provider);
      // A form with several questions shows the next one without a status change.
      rereadRequest(pane.id, 600);
      return reply({ t: "ack", ok: true });
    }
    case "approve":
    case "decline":
    case "interrupt": {
      const approval = approvalFor(pane.id);
      if (!approval) break;
      answerApproval(approval.id, req.t === "approve");
      logActivity(pane.workspaceId, `${req.t === "approve" ? "Approved" : "Declined"} “${approval.title}” from your phone`, pane.provider);
      return reply({ t: "ack", ok: true });
    }
  }
  switch (req.t) {
    case "approve":
      execute({ type: "pane.approve", target });
      logActivity(pane.workspaceId, "Approved from your phone", pane.provider);
      return reply({ t: "ack", ok: true });
    case "decline":
    case "interrupt":
      // Esc cancels the pending request in Claude Code and Codex, and stops a running turn.
      execute({ type: "pane.interrupt", target });
      logActivity(pane.workspaceId, req.t === "decline" ? "Declined from your phone" : "Interrupted from your phone", pane.provider);
      return reply({ t: "ack", ok: true });
    default:
      return reply({ t: "error", error: `unknown request ${req.t}` });
  }
}

/** Starts an agent session in a workspace for the phone; its first message is optional. */
function newPane(req: Request): Record<string, unknown> {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === req.workspaceId);
  if (!ws) return { t: "ack", ok: false, error: "That workspace is gone." };
  const providerId = req.provider && req.provider !== "default" ? req.provider : defaultProviderId(s);
  const provider = s.providers.find((p) => p.id === providerId && p.installed && p.id !== "shell");
  if (!provider) return { t: "ack", ok: false, error: "That agent isn't installed on the desktop." };
  const prompt = req.text?.trim() || undefined;
  const id = s.addPane(provider.id, { workspaceId: ws.id, prompt });
  // Started right away: the desktop may be minimized or show another workspace, and its
  // tile only starts a pane once it's drawn. The tile adopts the running terminal later.
  const pane = allPanes().find((p) => p.id === id);
  if (pane) startDetached(pane);
  logActivity(ws.id, `Started ${provider.name} from your phone${prompt ? `: ${prompt.slice(0, 80)}` : ""}`, provider.id);
  return { t: "ack", ok: true, paneId: id };
}

/** Reads a waiting pane's request from its screen again and tells the phones when it changed. */
function rereadRequest(id: string, delay: number) {
  setTimeout(() => {
    const now = allPanes().find((p) => p.id === id);
    if (now?.status !== "needs-input") return;
    const next = describeRequest(readTerminal(id, 40) ?? "", messages.get(id) ?? undefined);
    if (JSON.stringify(next) === JSON.stringify(requests.get(id))) return;
    requests.set(id, next);
    broadcast(id);
  }, delay);
}

/** What the phones were last told about each pane (its snapshot, minus the time). */
const lastSent = new Map<string, string>();
const signature = (p: MobilePane) => JSON.stringify({ ...p, updatedAt: 0 });

/**
 * Tells the phones about a pane. A status change also pushes a notification from Rust;
 * `silent` updates (a new title, a moved pane) don't.
 */
function broadcast(paneId: string, silent = false) {
  const pane = allPanes().find((p) => p.id === paneId);
  if (!pane) return;
  const snap = snapshot(pane);
  lastSent.set(pane.id, signature(snap));
  void send(null, silent ? { t: "status", pane: snap, silent: true } : { t: "status", pane: snap });
}

/** The whole mirror, when panes come or go or the workspaces or agents change. */
function broadcastAll() {
  const all = mirror();
  for (const p of all.panes) lastSent.set(p.id, signature(p));
  void send(null, { t: "panes", ...all });
}

/** Which panes, workspaces and agents there are, in the desktop's order. */
function layoutKey(): string {
  const s = useWorkspaces.getState();
  return JSON.stringify([
    s.activeWorkspaceId,
    s.workspaces.map((w) => [w.id, w.name, ordered(w).map((p) => p.id)]),
    s.providers.filter((p) => p.installed).map((p) => p.id),
    s.defaultProvider,
  ]);
}

let syncTimer: ReturnType<typeof setTimeout> | null = null;

/** Sends the panes whose snapshot changed without a status change (title, workspace name…). */
function scheduleSync() {
  if (syncTimer) return;
  syncTimer = setTimeout(() => {
    syncTimer = null;
    for (const pane of allPanes()) {
      // A waiting pane goes out (with its push) once its question is read off the screen.
      if (pane.status === "needs-input" && !requests.has(pane.id) && !approvalFor(pane.id)) continue;
      const snap = snapshot(pane);
      if (lastSent.get(pane.id) !== signature(snap)) broadcast(pane.id, true);
    }
  }, 150);
}

let started = false;

/** Lets Rust tell "at the desktop" from "away": pushes wait while the user works in winfer. */
function trackActivity() {
  let last = 0;
  const seen = () => {
    const now = Date.now();
    if (now - last < 20_000) return;
    last = now;
    void invoke("mobile_activity").catch(() => {});
  };
  if (typeof window === "undefined") return;
  for (const type of ["keydown", "pointerdown", "wheel", "focus"] as const) window.addEventListener(type, seen, { capture: true, passive: true });
}

export function startMobileBridge() {
  if (started) return;
  started = true;
  trackActivity();
  void listen<{ device: string; msg: Request }>("mobile://request", ({ payload }) => void onRequest(payload.device, payload.msg));

  // Plugin approvals change what a pane shows on the phone.
  let asking = new Set<string>();
  useApprovals.subscribe(({ queue }) => {
    for (const a of queue) if (!approvalAt.has(a.id)) approvalAt.set(a.id, Date.now());
    for (const id of approvalAt.keys()) if (!queue.some((a) => a.id === id)) approvalAt.delete(id);
    const now = new Set(queue.flatMap((a) => (a.paneId ? [a.paneId] : [])));
    for (const id of new Set([...asking, ...now])) broadcast(id);
    asking = now;
  });

  const seen = new Map<string, AgentStatus>(allPanes().map((p) => [p.id, p.status]));
  let layout = layoutKey();
  useWorkspaces.subscribe((s) => {
    const panes = s.workspaces.flatMap((w) => w.panes);
    const before = layout;
    layout = layoutKey();
    for (const pane of panes) {
      if (seen.get(pane.id) === pane.status) continue;
      const known = seen.has(pane.id);
      seen.set(pane.id, pane.status);
      changedAt.set(pane.id, Date.now());
      requests.delete(pane.id);
      if (pane.status === "needs-input") {
        // The hook fires as the prompt appears; give the terminal a moment to draw it.
        rereadRequest(pane.id, 450);
      } else if (known) {
        broadcast(pane.id);
      }
    }
    const ids = new Set(panes.map((p) => p.id));
    for (const id of [...seen.keys()].filter((id) => !ids.has(id))) {
      seen.delete(id);
      messages.delete(id);
      changedAt.delete(id);
      requests.delete(id);
      lastSent.delete(id);
    }
    // New, closed or moved panes, renamed workspaces: the phones get the whole mirror again.
    if (layout !== before) broadcastAll();
    scheduleSync();
  });
}

/* Settings */

export interface MobileDevice {
  deviceId: string;
  name: string;
  createdAt: number;
  lastSeen: number | null;
  paired: boolean;
  expiresAt: number | null;
  push: boolean;
  connected: boolean;
  phones: number;
  error: string | null;
}

export interface MobileStatus {
  relayUrl: string;
  defaultRelayUrl: string;
  devices: MobileDevice[];
}

export interface Pairing {
  deviceId: string;
  link: string;
  qrSvg: string;
  expiresAt: number;
}

export const mobileStatus = () => invoke<MobileStatus>("mobile_status");
export const startPairing = () => invoke<Pairing>("mobile_pair_start");
export const revokeDevice = (deviceId: string) => invoke("mobile_revoke", { deviceId });
export const setRelayUrl = (url: string) => invoke("mobile_set_relay_url", { url });
export const onMobileChange = (f: () => void) => listen("mobile://changed", f);
