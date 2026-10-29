/** When an agent hits its usage limit: carry its task over to an agent that still has room. */
import { useProgress } from "@/features/progress/progress";
import { readTerminal } from "@/features/terminal/xtermRegistry";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { activeKey, leftNow, tightest, useUsage } from "./usage";

/** The installed agent with the most room left (OpenCode has no plan limits, so it's last). */
export function handoffTarget(from: string): string | null {
  const { providers } = useWorkspaces.getState();
  const { snapshots } = useUsage.getState();
  const candidates = providers
    .filter((p) => p.installed && p.id !== from && p.id !== "shell" && p.command)
    .map((p) => {
      const w = tightest(snapshots[activeKey(p.id)]);
      return { id: p.id, left: w ? leftNow(w) : p.id === "opencode" ? 50 : -1 };
    })
    .filter((c) => c.left >= 10)
    .sort((a, b) => b.left - a.left);
  return candidates[0]?.id ?? null;
}

/** The pane to hand over: the focused one if it's this agent's, else its most recently busy one. */
export function paneToHandOff(provider: string): Pane | null {
  const s = useWorkspaces.getState();
  const panes = s.workspaces.flatMap((w) => w.panes).filter((p) => p.provider === provider);
  return panes.find((p) => p.id === s.focusedPaneId) ?? panes.find((p) => p.status === "working" || p.status === "needs-input") ?? panes[0] ?? null;
}

export function handoffPrompt(pane: Pane, fromName: string): string {
  const progress = useProgress.getState().byPane[pane.id];
  const lines = [`${fromName} was working on this and hit its usage limit, so you're taking over.`];
  if (progress?.task) lines.push(`The task: ${progress.task}`);
  if (progress?.percent != null) lines.push(`It was about ${progress.percent}% done${progress.step ? ` (last step: ${progress.step})` : ""}.`);
  lines.push(
    pane.worktree
      ? `Its work so far is in this folder, on branch ${pane.worktree.branch}. Check git status, the diff and recent commits, then continue where it left off.`
      : "Its work so far is in this folder. Check git status and the diff, then continue where it left off.",
  );
  const tail = readTerminal(pane.id, 30)?.trim();
  if (tail) lines.push(`Its last output:\n${tail.slice(-3000)}`);
  return lines.join("\n\n");
}

/** Opens `to` next to the pane, in the same worktree (or folder), with the task. */
export function continueIn(pane: Pane, to: string) {
  const s = useWorkspaces.getState();
  const fromName = providerName(s.providers, pane.provider);
  if (s.activeWorkspaceId !== pane.workspaceId) s.setActiveWorkspace(pane.workspaceId);
  s.addPane(to, {
    workspaceId: pane.workspaceId,
    splitFrom: pane.id,
    prompt: handoffPrompt(pane, fromName),
    ...(pane.worktree ? { existingWorktree: pane.worktree } : { worktree: false, cwd: pane.cwd }),
  });
}
