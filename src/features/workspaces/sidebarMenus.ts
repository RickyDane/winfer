/** Right-click menus for the sidebar's workspaces, agents and worktrees. */
/** Opens the branch picker for `workspace`'s repo (it works on the active workspace). */
export function openBranchDialog(workspace: Workspace) {
  const s = useWorkspaces.getState();
  if (s.activeWorkspaceId !== workspace.id) s.setActiveWorkspace(workspace.id);
  s.setOverlay("branch");
}

import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { execute } from "@/commands/bus";
import { IS_MAC, keysFor } from "@/commands/keymap";
import type { MenuItem } from "@/components/ContextMenu";
import { reviewFor } from "@/features/review/reviews";
import { restartTerminal } from "@/features/terminal/xtermRegistry";
import { getHome } from "@/lib/home";
import { defaultProviderId, providerName, useWorkspaces, type Pane, type Workspace } from "@/stores/workspaces";
import type { ExistingWorktree } from "@/features/worktree/worktree";

const REVEAL = IS_MAC ? "Reveal in Finder" : "Show in Explorer";
const reveal = (path: string) => void revealItemInDir(path.replace(/^~(?=[/\\]|$)/, getHome())).catch(() => {});
const copy = (text: string) => void navigator.clipboard.writeText(text);

export function workspaceMenu(ws: Workspace): MenuItem[] {
  const agent = providerName(useWorkspaces.getState().providers, defaultProviderId());
  return [
    { label: "Open", run: () => execute({ type: "workspace.switch", target: { id: ws.id } }) },
    {
      label: `New ${agent} here`,
      run: () => {
        execute({ type: "workspace.switch", target: { id: ws.id } });
        execute({ type: "pane.spawn", provider: "default" });
      },
    },
    { label: "New worktree from a branch…", run: () => openBranchDialog(ws) },
    "separator",
    { label: REVEAL, run: () => reveal(ws.rootDir) },
    { label: "Copy folder path", run: () => copy(ws.rootDir) },
    "separator",
    { label: "Remove workspace…", run: () => execute({ type: "workspace.remove", id: ws.id }), danger: true },
  ];
}

export function paneMenu(pane: Pane): MenuItem[] {
  const s = useWorkspaces.getState();
  const provider = s.providers.find((p) => p.id === pane.provider);
  const target = { id: pane.id };
  const isAgent = pane.provider !== "shell";
  const review = reviewFor(pane.id);
  const wt = pane.worktree;
  return [
    { label: "Focus", run: () => execute({ type: "pane.focus", target }) },
    { label: "Zoom", hint: keysFor({ type: "pane.zoom", target: "focused" }), run: () => execute({ type: "pane.zoom", target }) },
    {
      label: "Show in overview",
      run: () => {
        execute({ type: "pane.focus", target });
        execute({ type: "ui.view", view: "overview" });
      },
    },
    "separator",
    ...(pane.status === "needs-input" ? [{ label: "Approve request", run: () => execute({ type: "pane.approve", target }) }] : []),
    { label: isAgent ? "Interrupt" : "Interrupt (Ctrl-C)", hint: keysFor({ type: "pane.interrupt", target: "focused" }), run: () => execute({ type: "pane.interrupt", target }) },
    ...(isAgent
      ? [
          { label: "Restart agent (resume session)", run: () => {
              s.updatePane(pane.id, { resume: provider?.resumeArgs.length ? true : false, prompt: null });
              const fresh = useWorkspaces.getState().workspaces.flatMap((w) => w.panes).find((p) => p.id === pane.id);
              if (fresh) restartTerminal(fresh);
            } },
          ...(provider?.modelArg
            ? [{ label: "Change model…", run: () => {
                  execute({ type: "pane.focus", target });
                  execute({ type: "ui.overlay", overlay: "model" });
                } }]
            : []),
        ]
      : []),
    "separator",
    ...(wt
      ? [
          { label: "Show changes", run: () => execute({ type: "pane.showChanges", target }) },
          ...(review ? [{ label: "Open its review", run: () => execute({ type: "ui.overlay", overlay: "review" }) }] : []),
          { label: `Merge into ${wt.base}…`, run: () => execute({ type: "pane.mergeWorktree", target }) },
          "separator" as const,
          { label: REVEAL, run: () => reveal(wt.path) },
          { label: "Copy worktree path", run: () => copy(wt.path) },
          { label: "Copy branch name", run: () => copy(wt.branch) },
        ]
      : [
          { label: REVEAL, run: () => reveal(pane.cwd) },
          { label: "Copy folder path", run: () => copy(pane.cwd) },
        ]),
    "separator",
    ...(wt ? [{ label: "Discard worktree…", run: () => execute({ type: "pane.discardWorktree", target }), danger: true }] : []),
    { label: "Close", hint: keysFor({ type: "pane.close", target: "focused" }), run: () => execute({ type: "pane.close", target }), danger: true },
  ];
}

/** Opens `provider` (or a shell) inside an existing worktree of `workspace`. */
export function openInWorktree(workspace: Workspace, wt: ExistingWorktree, provider: string) {
  const s = useWorkspaces.getState();
  if (s.activeWorkspaceId !== workspace.id) s.setActiveWorkspace(workspace.id);
  // In the project folder itself the agent works directly on its branch, without a worktree.
  if (wt.isMain) s.addPane(provider, { workspaceId: workspace.id, worktree: false, cwd: wt.info.path });
  else s.addPane(provider, { workspaceId: workspace.id, existingWorktree: wt.info });
}

export function worktreeMenu(workspace: Workspace, wt: ExistingWorktree, remove: () => void): MenuItem[] {
  const s = useWorkspaces.getState();
  const agents = s.providers.filter((p) => p.installed && p.id !== "shell");
  const missing = wt.prunable;
  return [
    ...agents.map((p) => ({ label: `Open ${p.name} here`, disabled: missing, run: () => openInWorktree(workspace, wt, p.id) })),
    { label: "Open a shell here", disabled: missing, run: () => openInWorktree(workspace, wt, "shell") },
    "separator",
    { label: REVEAL, disabled: missing, run: () => reveal(wt.info.path) },
    { label: "Copy worktree path", run: () => copy(wt.info.path) },
    { label: "Copy branch name", run: () => copy(wt.info.branch) },
    "separator",
    { label: "New worktree from a branch…", run: () => openBranchDialog(workspace) },
    ...(wt.isMain ? [] : ["separator" as const, { label: missing ? "Forget missing worktree…" : "Remove worktree…", run: remove, danger: true }]),
  ];
}
