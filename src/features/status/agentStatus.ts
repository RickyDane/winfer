/**
 * Agent status: hook events from Claude Code / Codex / OpenCode 2 (src-tauri/src/hooks.rs), terminal
 * notifications (OSC 9/777, bell) and, for agents without hooks, output activity.
 */
import { listen } from "@tauri-apps/api/event";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { providerName, useWorkspaces, type AgentStatus, type Pane } from "@/stores/workspaces";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { outputAge, readTerminal } from "@/features/terminal/xtermRegistry";
import { execute } from "@/commands/bus";
import { describeRequest } from "./request";
import { setLastReply } from "./replies";
import { logActivity } from "@/stores/activity";
import { speak } from "@/features/voice/readback";
import { agentFinished } from "@/features/review/reviews";
import { estimateNow, markFinished } from "@/features/progress/progress";

const IDLE_AFTER_MS = 4000;

const findPane = (id: string): Pane | undefined =>
  useWorkspaces.getState().workspaces.flatMap((w) => w.panes).find((p) => p.id === id);

const hasHooks = (pane: Pane) => useWorkspaces.getState().providers.find((p) => p.id === pane.provider)?.statusHooks ?? false;

let permission: Promise<boolean> | null = null;
async function canNotify() {
  permission ??= isPermissionGranted().then((granted) => granted || requestPermission().then((r) => r === "granted"));
  return permission;
}

async function notify(pane: Pane, status: AgentStatus, message?: string) {
  const s = useWorkspaces.getState();
  if (document.hasFocus() && s.focusedPaneId === pane.id) return;
  // Auto-approved agents don't stop for permission prompts: no Approve / Decline notifications.
  if (status === "needs-input" && s.autoApprove.includes(pane.provider)) return;
  if (!(await canNotify())) return;
  const workspace = s.workspaces.find((w) => w.id === pane.workspaceId)?.name ?? "";
  const name = providerName(s.providers, pane.provider);
  if (status === "needs-input" && (await notifyRequest(pane, name, workspace, message))) return;
  const title = status === "needs-input" ? `${name} needs your input` : status === "error" ? `${name} exited with an error` : `${name} finished`;
  sendNotification({ title, body: message ? `${workspace}: ${message}` : workspace });
}

/**
 * A notification with the agent's actual question and buttons to answer it (Approve /
 * Decline, or a reply field for open questions on macOS). False if it couldn't be shown.
 */
async function notifyRequest(pane: Pane, name: string, workspace: string, hookMessage?: string): Promise<boolean> {
  // The hook fires as the prompt appears; give the terminal a moment to draw it.
  await new Promise((r) => setTimeout(r, 400));
  const request = describeRequest(readTerminal(pane.id, 40) ?? "", hookMessage);
  const title = `${name} ${request.kind === "approval" ? "asks for permission" : "is waiting for you"}${workspace ? ` · ${workspace}` : ""}`;
  try {
    await invoke("notify_action", { notification: { paneId: pane.id, title, body: request.text, kind: request.kind } });
    return true;
  } catch {
    return false;
  }
}

/** Runs what the user chose in an actionable notification. */
function onNotificationAction({ paneId, action, text }: { paneId: string; action: "approve" | "decline" | "reply" | "open"; text: string | null }) {
  const pane = findPane(paneId);
  if (!pane) return;
  const name = providerName(useWorkspaces.getState().providers, pane.provider);
  const target = { id: paneId };
  if (action === "approve") {
    execute({ type: "pane.approve", target });
    logActivity(pane.workspaceId, `Approved ${name} from a notification`, pane.provider);
  } else if (action === "decline") {
    // Esc cancels the pending request in Claude Code and Codex.
    execute({ type: "pane.interrupt", target });
    logActivity(pane.workspaceId, `Declined ${name} from a notification`, pane.provider);
  } else if (action === "reply" && text) {
    execute({ type: "pane.sendText", target, text, submit: true });
    logActivity(pane.workspaceId, `Answered ${name}: ${text.slice(0, 80)}`, pane.provider);
  } else {
    const window = getCurrentWindow();
    void window.unminimize().then(() => window.show()).then(() => window.setFocus());
    execute({ type: "pane.focus", target });
  }
}

/** Single entry point for status changes; notifies on transitions that need the user. */
export function reportStatus(paneId: string, status: AgentStatus, message?: string) {
  const pane = findPane(paneId);
  if (!pane || pane.status === status) return;
  useWorkspaces.getState().setPaneStatus(paneId, status);
  // Hook agents report "done"; the others go quiet (working → idle).
  if (status === "done") {
    agentFinished(paneId);
    markFinished(paneId);
  } else if (status === "idle" && pane.status === "working" && !hasHooks(pane)) {
    // Agents without completion hooks only go quiet, which also happens right after they start
    // or while they wait for an answer: let the LLM judge whether the task is done.
    agentFinished(paneId);
    estimateNow(paneId);
  }
  if (status === "needs-input" || status === "done" || status === "error") {
    void notify(pane, status, message);
    const name = providerName(useWorkspaces.getState().providers, pane.provider);
    const verb = status === "needs-input" ? "needs your input" : status === "done" ? "finished" : "stopped with an error";
    const detail = message && status === "needs-input" ? (message.startsWith(name) ? message : `${name}: ${message}`) : `${name} ${verb}`;
    logActivity(pane.workspaceId, detail, pane.provider);
    if (useWorkspaces.getState().focusedPaneId !== pane.id || !document.hasFocus()) speak(`${name} ${verb}`);
  }
}

/**
 * A terminal notification (OSC 9/777, bell). Agents with hooks report requests through them,
 * so a beep after they finished is only an idle reminder, not a new question.
 */
export function reportTerminalNotice(paneId: string, message?: string) {
  const pane = findPane(paneId);
  if (!pane || pane.provider === "shell") return;
  if (pane.status === "done" && hasHooks(pane)) return;
  reportStatus(paneId, "needs-input", message);
}

/** The user submitted input to an agent: it's working now. */
export function reportInput(paneId: string) {
  const pane = findPane(paneId);
  if (pane && pane.provider !== "shell") reportStatus(paneId, "working");
}

/** Output arrived from an agent without hooks. */
export function reportOutput(paneId: string) {
  const pane = findPane(paneId);
  if (pane && pane.provider !== "shell" && !hasHooks(pane) && pane.status !== "working") {
    useWorkspaces.getState().setPaneStatus(paneId, "working");
  }
}

let started = false;

export function startStatusTracking() {
  if (started) return;
  started = true;
  void listen<Parameters<typeof onNotificationAction>[0]>("notification://action", ({ payload }) => onNotificationAction(payload));
  void listen<{ paneId: string; status: AgentStatus; message?: string; reply?: string | null; session?: string | null }>("agent://status", ({ payload }) => {
    // Remembered so the pane (or its worktree, reopened later) resumes this very session.
    if (payload.session) useWorkspaces.getState().setPaneSession(payload.paneId, payload.session);
    // Set before the status change so plugins see the reply with the "done" event.
    if (payload.status === "done") setLastReply(payload.paneId, payload.reply);
    reportStatus(payload.paneId, payload.status, payload.message ?? undefined);
  });

  // Agents without hooks: quiet output means they're waiting.
  window.setInterval(() => {
    for (const pane of useWorkspaces.getState().workspaces.flatMap((w) => w.panes)) {
      if (pane.status === "working" && pane.provider !== "shell" && !hasHooks(pane) && outputAge(pane.id) > IDLE_AFTER_MS) {
        reportStatus(pane.id, "idle");
      }
    }
  }, 1000);
}
