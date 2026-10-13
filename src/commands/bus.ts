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
import { closeTerminal } from "@/features/terminal/xtermRegistry";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { useWorkspaces, type Workspace } from "@/stores/workspaces";

export type Direction = "left" | "right" | "up" | "down";

/** "focused", a pane id, or a 1-based position in layout order (what the pane badges show). */
export type PaneRef = "focused" | { id: string } | { index: number };

export type Command =
  | { type: "pane.spawn"; provider: ProviderId | "default"; count?: number }
  | { type: "pane.split"; dir: SplitDir; provider?: ProviderId | "default" }
  | { type: "pane.close"; target: PaneRef }
  | { type: "pane.focus"; target: PaneRef }
  | { type: "pane.focusDir"; dir: Direction }
  | { type: "pane.swapDir"; dir: Direction }
  | { type: "pane.resize"; target: PaneRef; axis: SplitDir; delta: number }
  | { type: "pane.zoom"; target: PaneRef }
  | { type: "layout.equalize" }
  | { type: "layout.preset"; name: Preset | "next" }
  | { type: "workspace.switch"; target: { id: string } | { index: number } }
  /** Without a rootDir, asks for a folder. */
  | { type: "workspace.create"; rootDir?: string; name?: string }
  | { type: "workspace.remove"; id: string }
  | { type: "ui.toggleSidebar" }
  | { type: "ui.toggleBrowser" }
  | { type: "ui.palette"; query?: string }
  | { type: "ui.cheatSheet" };

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
      for (let i = 0; i < Math.max(1, cmd.count ?? 1); i++) s.addPane(provider);
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
    case "ui.toggleSidebar":
      return s.toggleSidebar();
    case "ui.toggleBrowser":
      return s.toggleBrowser();
    case "ui.palette":
      return s.paletteQuery === null ? s.openPalette(cmd.query) : s.closePalette();
    case "ui.cheatSheet":
      return s.setCheatSheet(!s.cheatSheetOpen);
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
