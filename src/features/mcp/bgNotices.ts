/**
 * Exit notices for background tasks agents start with winfer's `bg` tool (src-tauri/src/mcp.rs):
 * when one exits without the agent having seen it, winfer types a note into the agent's pane,
 * once the agent is idle so it doesn't land in a running turn or an approval prompt.
 */
import { listen } from "@tauri-apps/api/event";
import { pasteText, sendText } from "@/features/terminal/xtermRegistry";
import { useWorkspaces } from "@/stores/workspaces";

let queued = new Map<string, string[]>();

export function queueBgNote(paneId: string, text: string) {
  queued.set(paneId, [...(queued.get(paneId) ?? []), text]);
}

export function pendingBgNotes(paneId: string): string[] {
  return queued.get(paneId) ?? [];
}

/** Types the queued notes into panes whose agent is idle; drops those of panes that are gone. */
export function deliverBgNotes() {
  if (!queued.size) return;
  const panes = new Map(useWorkspaces.getState().workspaces.flatMap((w) => w.panes.map((p) => [p.id, p] as const)));
  const next = new Map<string, string[]>();
  for (const [paneId, notes] of queued) {
    const pane = panes.get(paneId);
    if (!pane) continue;
    if (pane.status === "working" || pane.status === "needs-input") {
      next.set(paneId, notes);
      continue;
    }
    const text = notes.join("\n\n");
    if (text.includes("\n")) pasteText(paneId, text);
    else sendText(paneId, text);
    sendText(paneId, "\r");
  }
  queued = next;
}

let started = false;

export function startBgNotices() {
  if (started) return;
  started = true;
  void listen<{ paneId: string; text: string }>("bg://note", ({ payload }) => {
    queueBgNote(payload.paneId, payload.text);
    deliverBgNotes();
  });
  window.setInterval(() => deliverBgNotes(), 2000);
}
