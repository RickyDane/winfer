/**
 * Agent status: hook events from Claude Code / Codex (src-tauri/src/hooks.rs), terminal
 * notifications (OSC 9/777, bell) and, for agents without hooks, output activity.
 */
import { listen } from "@tauri-apps/api/event";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { providerName, useWorkspaces, type AgentStatus, type Pane } from "@/stores/workspaces";
import { outputAge } from "@/features/terminal/xtermRegistry";
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
  if (!(await canNotify())) return;
  const workspace = s.workspaces.find((w) => w.id === pane.workspaceId)?.name ?? "";
  const name = providerName(s.providers, pane.provider);
  const title = status === "needs-input" ? `${name} needs your input` : status === "error" ? `${name} exited with an error` : `${name} finished`;
  sendNotification({ title, body: message ? `${workspace}: ${message}` : workspace });
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
  void listen<{ paneId: string; status: AgentStatus; message?: string }>("agent://status", ({ payload }) =>
    reportStatus(payload.paneId, payload.status, payload.message ?? undefined),
  );

  // Agents without hooks: quiet output means they're waiting.
  window.setInterval(() => {
    for (const pane of useWorkspaces.getState().workspaces.flatMap((w) => w.panes)) {
      if (pane.status === "working" && pane.provider !== "shell" && !hasHooks(pane) && outputAge(pane.id) > IDLE_AFTER_MS) {
        reportStatus(pane.id, "idle");
      }
    }
  }, 1000);
}
