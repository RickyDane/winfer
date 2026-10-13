/**
 * The Command Bus. Every input — keyboard, palette, buttons, and later voice and MCP —
 * becomes one of these typed commands and runs through `execute`.
 */
import type { ProviderId } from "@/features/providers/providers";
import {
  buildPreset,
  equalize,
  leaves,
  PRESETS,
  resizePane,
  swapLeaves,
  type LayoutNode,
  type Preset,
  type SplitDir,
} from "@/features/layout/tree";
import { closeTerminal, sendText } from "@/features/terminal/xtermRegistry";
import { ask, message, open } from "@tauri-apps/plugin-dialog";
import { mergeWorktree, removeWorktree } from "@/features/worktree/worktree";
import * as browser from "@/features/browser/browser";
import { loadRecipe } from "@/features/recipes/recipes";
import { providerName, useWorkspaces, type Workspace, type WorktreeInfo } from "@/stores/workspaces";
import { logActivity } from "@/stores/activity";

export type Direction = "left" | "right" | "up" | "down";

/** "focused", a pane id, or a 1-based position in layout order (what the pane badges show). */
export type PaneRef = "focused" | { id: string } | { index: number };

export type Command =
  | { type: "pane.spawn"; provider: ProviderId | "default"; count?: number; prompt?: string; worktree?: boolean }
  | { type: "pane.split"; dir: SplitDir; provider?: ProviderId | "default" }
  | { type: "pane.close"; target: PaneRef }
  | { type: "pane.focus"; target: PaneRef }
  | { type: "pane.focusDir"; dir: Direction }
  | { type: "pane.swapDir"; dir: Direction }
  | { type: "pane.resize"; target: PaneRef; axis: SplitDir; delta: number }
  | { type: "pane.zoom"; target: PaneRef }
  /** Closes every pane in the active workspace. */
  | { type: "pane.closeAll" }
  /** Stops what an agent is doing (Esc; Ctrl-C for shells). */
  | { type: "pane.interrupt"; target: PaneRef }
  /** Accepts the agent's pending permission prompt. */
  | { type: "pane.approve"; target: PaneRef }
  /** Sends the same text to every agent pane in the workspace. */
  | { type: "pane.broadcast"; text: string; submit?: boolean; paneIds?: string[] }
  /** Race mode: start several agents on the same prompt, each in its own worktree. */
  | { type: "race.start"; counts: Record<ProviderId, number>; prompt: string }
  /** Toggles the scrollback search bar of the focused pane. */
  | { type: "pane.search" }
  /** Types text into a pane's terminal (and presses Enter if `submit`). */
  | { type: "pane.sendText"; target: PaneRef; text: string; submit?: boolean }
  | { type: "pane.showChanges"; target: PaneRef }
  | { type: "pane.mergeWorktree"; target: PaneRef }
  | { type: "pane.discardWorktree"; target: PaneRef }
  /** Jump to the next pane (any workspace) whose agent needs input or finished unseen. */
  | { type: "pane.nextAttention" }
  | { type: "layout.equalize" }
  | { type: "layout.preset"; name: Preset | "next" }
  | { type: "workspace.switch"; target: { id: string } | { index: number } }
  /** Without a rootDir, asks for a folder. */
  | { type: "workspace.create"; rootDir?: string; name?: string }
  | { type: "workspace.remove"; id: string }
  | { type: "workspace.toggleWorktrees" }
  | { type: "browser.open"; url: string; newTab?: boolean }
  | { type: "browser.newTab" }
  | { type: "browser.closeTab" }
  | { type: "browser.history"; delta: -1 | 0 | 1 }
  | { type: "browser.device"; device: browser.Device | "toggle" }
  | { type: "browser.focusUrl" }
  /** Element picker: click an element in the page to paste its details into the agent. */
  | { type: "browser.pick" }
  | { type: "browser.sendConsole" }
  | { type: "ui.toggleSidebar" }
  | { type: "ui.toggleBrowser" }
  | { type: "ui.palette"; query?: string }
  | { type: "ui.cheatSheet" }
  | { type: "ui.settings" }
  | { type: "ui.overlay"; overlay: "race" | "broadcast" | "recipes" }
  | { type: "recipe.load"; id: string };

const store = () => useWorkspaces.getState();
const active = (): Workspace => {
  const s = store();
  return s.workspaces.find((w) => w.id === s.activeWorkspaceId)!;
};

/** Default provider for ⌘T: Claude Code if installed, else the shell. */
function resolveProvider(provider: ProviderId | "default"): ProviderId {
  if (provider !== "default") return provider;
  return store().providers.find((p) => p.id === "claude" && p.installed) ? "claude" : "shell";
}

const paneById = (id: string | null) => (id ? active().panes.find((p) => p.id === id) : undefined);

export function resolvePane(target: PaneRef): string | null {
  const ws = active();
  const ids = ws.layout ? leaves(ws.layout) : [];
  if (target === "focused") return store().focusedPaneId && ids.includes(store().focusedPaneId!) ? store().focusedPaneId : null;
  if ("id" in target) return ids.includes(target.id) ? target.id : null;
  return ids[target.index - 1] ?? null;
}

/** Nearest visible pane in a direction, using on-screen geometry. */
function paneInDirection(fromId: string, dir: Direction): string | null {
  const rects = new Map<string, DOMRect>();
  document.querySelectorAll<HTMLElement>("[data-pane-id]").forEach((el) => {
    rects.set(el.dataset.paneId!, el.getBoundingClientRect());
  });
  const from = rects.get(fromId);
  if (!from) return null;
  const cx = from.left + from.width / 2;
  const cy = from.top + from.height / 2;

  let best: { id: string; score: number } | null = null;
  for (const [id, r] of rects) {
    if (id === fromId) continue;
    const ok =
      (dir === "left" && r.right <= from.left + 1) ||
      (dir === "right" && r.left >= from.right - 1) ||
      (dir === "up" && r.bottom <= from.top + 1) ||
      (dir === "down" && r.top >= from.bottom - 1);
    if (!ok) continue;
    const dx = r.left + r.width / 2 - cx;
    const dy = r.top + r.height / 2 - cy;
    // Prefer panes aligned with the movement axis.
    const score = dir === "left" || dir === "right" ? Math.abs(dx) + 2 * Math.abs(dy) : Math.abs(dy) + 2 * Math.abs(dx);
    if (!best || score < best.score) best = { id, score };
  }
  return best?.id ?? null;
}

export function execute(cmd: Command): void {
  const s = store();
  switch (cmd.type) {
    case "pane.spawn": {
      const provider = resolveProvider(cmd.provider);
      const count = Math.max(1, cmd.count ?? 1);
      for (let i = 0; i < count; i++) s.addPane(provider, { prompt: cmd.prompt, worktree: cmd.worktree });
      logActivity(s.activeWorkspaceId, `Opened ${count > 1 ? `${count}× ` : ""}${providerName(s.providers, provider)}`, provider);
      return;
    }
    case "pane.closeAll": {
      const ws = active();
      if (ws.panes.length === 0) return;
      for (const pane of [...ws.panes]) {
        closeTerminal(pane.id);
        s.removePane(pane.id);
      }
      logActivity(ws.id, `Closed all ${ws.panes.length} panes`);
      return;
    }
    case "pane.interrupt": {
      const pane = paneById(resolvePane(cmd.target));
      if (pane) sendText(pane.id, pane.provider === "shell" ? "\x03" : "\x1b");
      return;
    }
    case "pane.approve": {
      const pane = paneById(resolvePane(cmd.target));
      if (pane) sendText(pane.id, s.providers.find((p) => p.id === pane.provider)?.approveInput ?? "\r");
      return;
    }
    case "pane.broadcast": {
      const targets = cmd.paneIds ?? active().panes.filter((p) => p.provider !== "shell").map((p) => p.id);
      targets.forEach((id) => sendText(id, cmd.text + (cmd.submit === false ? "" : "\r")));
      return;
    }
    case "race.start": {
      const raceId = `race-${crypto.randomUUID().slice(0, 8)}`;
      for (const [provider, count] of Object.entries(cmd.counts)) {
        for (let i = 0; i < count; i++) s.addPane(provider, { prompt: cmd.prompt, worktree: true, raceId });
      }
      s.updateActive((w) => ({ layout: w.layout && buildPreset("grid", leaves(w.layout)) }));
      s.setOverlay("race");
      const racers = Object.values(cmd.counts).reduce((a, b) => a + b, 0);
      logActivity(s.activeWorkspaceId, `Race started with ${racers} agents: ${cmd.prompt}`);
      return;
    }
    case "pane.search": {
      const id = resolvePane("focused");
      s.setSearchPane(s.searchPaneId === id ? null : id);
      return;
    }
    case "pane.split": {
      const from = resolvePane("focused");
      const id = s.addPane(resolveProvider(cmd.provider ?? "default"), { splitFrom: from ?? undefined });
      // addPane alternates direction by depth; force the requested direction on the new split.
      if (from) {
        s.updateActive((w) => ({ layout: w.layout && forceSplitDir(w.layout, id, cmd.dir) }));
      }
      return;
    }
    case "pane.close": {
      const id = resolvePane(cmd.target);
      if (!id) return;
      const closing = paneById(id);
      if (closing) logActivity(closing.workspaceId, `Closed ${providerName(s.providers, closing.provider)}`, closing.provider);
      closeTerminal(id);
      s.removePane(id);
      return;
    }
    case "pane.focus": {
      const id = resolvePane(cmd.target);
      if (id) s.focusPane(id);
      return;
    }
    case "pane.focusDir": {
      const from = resolvePane("focused");
      const to = from && paneInDirection(from, cmd.dir);
      if (to) s.focusPane(to);
      return;
    }
    case "pane.swapDir": {
      const from = resolvePane("focused");
      const to = from && paneInDirection(from, cmd.dir);
      if (from && to) s.updateActive((w) => ({ layout: w.layout && swapLeaves(w.layout, from, to) }));
      return;
    }
    case "pane.resize": {
      const id = resolvePane(cmd.target);
      if (id) s.updateActive((w) => ({ layout: w.layout && resizePane(w.layout, id, cmd.axis, cmd.delta) }));
      return;
    }
    case "pane.zoom": {
      const id = resolvePane(cmd.target);
      if (!id) return;
      s.focusPane(id);
      s.updateActive((w) => ({ zoomedPaneId: w.zoomedPaneId === id ? null : id }));
      return;
    }
    case "pane.sendText": {
      const id = resolvePane(cmd.target);
      if (id) sendText(id, cmd.text + (cmd.submit ? "\r" : ""));
      return;
    }
    case "pane.showChanges": {
      const pane = paneWithWorktree(cmd.target);
      if (pane) s.setDiffPane(pane.id);
      return;
    }
    case "pane.mergeWorktree": {
      const pane = paneWithWorktree(cmd.target);
      if (pane) void mergePane(pane);
      return;
    }
    case "pane.discardWorktree": {
      const pane = paneWithWorktree(cmd.target);
      if (pane) void discardPane(pane);
      return;
    }
    case "pane.nextAttention": {
      const all = s.workspaces.flatMap((w) => (w.layout ? leaves(w.layout) : []).map((id) => w.panes.find((p) => p.id === id)!));
      const rank = (p: (typeof all)[number]) => (p.status === "needs-input" ? 0 : p.unseen ? 1 : 2);
      // Skip the pane we're already on so repeated ⌘J cycles through everything waiting.
      const candidates = all.filter((p) => rank(p) < 2 && p.id !== s.focusedPaneId);
      if (candidates.length === 0) return;
      // Most urgent first; among equals, the next one after the focused pane (wrapping).
      const start = all.findIndex((p) => p.id === s.focusedPaneId);
      const distance = (p: (typeof all)[number]) => (all.indexOf(p) - start + all.length) % all.length || all.length;
      const next = candidates.sort((a, b) => rank(a) - rank(b) || distance(a) - distance(b))[0];
      if (next.workspaceId !== s.activeWorkspaceId) s.setActiveWorkspace(next.workspaceId);
      store().focusPane(next.id);
      return;
    }
    case "layout.equalize":
      s.updateActive((w) => ({ layout: w.layout && equalize(w.layout) }));
      return;
    case "layout.preset":
      s.updateActive((w) => {
        const presetIndex = cmd.name === "next" ? (w.presetIndex + 1) % PRESETS.length : PRESETS.indexOf(cmd.name);
        return {
          presetIndex,
          zoomedPaneId: null,
          layout: w.layout && buildPreset(PRESETS[presetIndex], leaves(w.layout)),
        };
      });
      return;
    case "workspace.switch": {
      const target = cmd.target;
      const ws = "id" in target ? s.workspaces.find((w) => w.id === target.id) : s.workspaces[target.index - 1];
      if (ws) s.setActiveWorkspace(ws.id);
      return;
    }
    case "workspace.create":
      void createWorkspace(cmd.rootDir, cmd.name);
      return;
    case "workspace.remove":
      void removeWorkspace(cmd.id);
      return;
    case "workspace.toggleWorktrees":
      s.updateActive((w) => ({ useWorktrees: !w.useWorktrees }));
      return;
    case "browser.open":
      void browser.openUrl(cmd.url, { newTab: cmd.newTab });
      return;
    case "browser.newTab":
      void browser.newTab();
      return;
    case "browser.closeTab":
      void browser.closeTab();
      return;
    case "browser.history":
      void browser.history(cmd.delta);
      return;
    case "browser.device":
      return browser.setDevice(cmd.device);
    case "browser.focusUrl":
      return browser.focusUrlBar();
    case "browser.pick":
      void browser.togglePick();
      return;
    case "browser.sendConsole":
      return browser.sendConsoleToAgent();
    case "ui.toggleSidebar":
      return s.toggleSidebar();
    case "ui.toggleBrowser":
      return s.toggleBrowser();
    case "ui.palette":
      return s.paletteQuery === null ? s.openPalette(cmd.query) : s.closePalette();
    case "ui.cheatSheet":
      return s.setCheatSheet(!s.cheatSheetOpen);
    case "ui.settings":
      return s.setSettings(!s.settingsOpen);
    case "recipe.load":
      return loadRecipe(cmd.id);
    case "ui.overlay":
      return s.setOverlay(s.overlay === cmd.overlay ? null : cmd.overlay);
  }
}

/** Sets the direction of the split that directly contains `paneId`. */
function forceSplitDir(node: LayoutNode, paneId: string, dir: SplitDir): LayoutNode {
  if (node.kind === "leaf") return node;
  if (node.children.some((c) => c.kind === "leaf" && c.paneId === paneId)) return { ...node, dir };
  return { ...node, children: node.children.map((c) => forceSplitDir(c, paneId, dir)) };
}

async function createWorkspace(rootDir?: string, name?: string) {
  const dir = rootDir ?? (await open({ directory: true, title: "Choose a folder for the new workspace" }));
  if (typeof dir !== "string") return;
  const base = dir.split(/[\\/]/).filter(Boolean).at(-1) ?? dir;
  store().createWorkspace(name ?? base, dir);
}

async function removeWorkspace(id: string) {
  const ws = store().workspaces.find((w) => w.id === id);
  if (!ws) return;
  if (ws.panes.length > 0) {
    const agents = ws.panes.length === 1 ? "1 pane" : `${ws.panes.length} panes`;
    const ok = await ask(`Remove workspace “${ws.name}”? Its ${agents} will be closed. Files and worktrees stay on disk.`, {
      title: "Remove workspace",
      kind: "warning",
      okLabel: "Remove",
    });
    if (!ok) return;
  }
  ws.panes.forEach((p) => closeTerminal(p.id));
  store().removeWorkspace(id);
}

function paneWithWorktree(target: PaneRef) {
  const id = resolvePane(target);
  const pane = active().panes.find((p) => p.id === id);
  if (pane?.worktree) return pane as typeof pane & { worktree: NonNullable<typeof pane.worktree> };
  if (pane) void message("This pane isn't running in its own worktree.", { title: "No worktree", kind: "info" });
  return null;
}

async function mergePane(pane: { worktree: WorktreeInfo }) {
  const { branch, base } = pane.worktree;
  const ok = await ask(`Merge ${branch} into ${base}?`, { title: "Merge worktree", okLabel: "Merge" });
  if (!ok) return;
  try {
    await mergeWorktree(pane.worktree);
    logActivity(active().id, `Merged ${branch} into ${base}`);
    await message(`Merged ${branch} into ${base}.`, { title: "Merged", kind: "info" });
  } catch (err) {
    await message(String(err), { title: "Merge not possible", kind: "error" });
  }
}

async function discardPane(pane: { id: string; worktree: WorktreeInfo }) {
  const { branch, path } = pane.worktree;
  const ok = await ask(`Close this pane, delete ${path} and the branch ${branch}? Unmerged work is lost.`, {
    title: "Discard worktree",
    kind: "warning",
    okLabel: "Discard",
  });
  if (!ok) return;
  closeTerminal(pane.id);
  store().removePane(pane.id);
  logActivity(active().id, `Discarded ${branch}`);
  try {
    await removeWorktree(pane.worktree, true);
  } catch (err) {
    await message(String(err), { title: "Couldn't remove worktree", kind: "error" });
  }
}
