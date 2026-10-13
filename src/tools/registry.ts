/**
 * Tool Registry: every user-facing action, described once.
 *
 * - `description` is what the Clef decision model sees as the option text (keep ~10–15 words).
 * - `params` map to System One question types (choice / noul / score) when voice lands in M5.
 * - `palette` expands a tool into concrete ⌘K entries.
 * - `toCommands` turns resolved arguments into Command Bus commands.
 */
import type { Command, PaneRef } from "@/commands/bus";
import type { Provider } from "@/features/providers/providers";
import { leaves, PRESETS, type Preset } from "@/features/layout/tree";
import { providerName, type Workspace } from "@/stores/workspaces";

export interface ToolContext {
  providers: Provider[];
  workspaces: Workspace[];
  active: Workspace;
}

type Options = (ctx: ToolContext) => Record<string, string | null>;

export type ParamDef =
  | { kind: "enum"; ask: string; options: Options } // → choice
  | { kind: "count"; ask: string; max: number } // → choice "zero".."max"
  | { kind: "perOption"; ask: string; options: Options; max: number } // → one count question per option
  | { kind: "bool"; ask: string } // → noul
  | { kind: "level"; ask: string; levels: string[] } // → score
  | { kind: "span"; ask: string }; // → choice over transcript spans

export type Args = Record<string, unknown>;

export interface PaletteEntry {
  title: string;
  args: Args;
}

export interface ToolDef {
  id: string;
  /** Short label used in the palette and HUD. */
  title: string;
  description: string;
  risk: "safe" | "confirm";
  params: Record<string, ParamDef>;
  toCommands(args: Args, ctx: ToolContext): Command[];
  palette?(ctx: ToolContext): PaletteEntry[];
}

const installedProviders: Options = (ctx) =>
  Object.fromEntries(ctx.providers.filter((p) => p.installed).map((p) => [p.id, p.name]));

/** Pane targets: "focused" plus every pane by id, described by number and provider. */
const paneTargets: Options = (ctx) => ({
  focused: "The pane that currently has focus",
  ...Object.fromEntries(
    (ctx.active.layout ? leaves(ctx.active.layout) : []).map((id, i) => {
      const pane = ctx.active.panes.find((p) => p.id === id)!;
      return [id, `Pane ${i + 1}: ${providerName(ctx.providers, pane.provider)}, ${pane.status}`];
    }),
  ),
});

const target = (args: Args): PaneRef =>
  args.target && args.target !== "focused" ? { id: String(args.target) } : "focused";

const RESIZE_STEP: Record<string, number> = { slightly: 0.05, moderately: 0.1, "a lot": 0.2 };
const DIRECTIONS = { left: "Left", right: "Right", up: "Up", down: "Down" } as const;

export const TOOLS: ToolDef[] = [
  {
    id: "spawn_panes",
    title: "New pane",
    description: "Open one or more new terminal panes running agent CLIs",
    risk: "safe",
    params: {
      counts: { kind: "perOption", ask: "How many new panes of this provider are requested?", options: installedProviders, max: 4 },
    },
    toCommands: (args) =>
      Object.entries((args.counts as Record<string, number>) ?? {})
        .filter(([, n]) => n > 0)
        .map(([provider, count]) => ({ type: "pane.spawn", provider, count })),
    palette: (ctx) =>
      ctx.providers
        .filter((p) => p.installed)
        .map((p) => ({ title: `New pane: ${p.name}`, args: { counts: { [p.id]: 1 } } })),
  },
  {
    id: "split_pane",
    title: "Split pane",
    description: "Split the focused pane and open the default agent beside or below it",
    risk: "safe",
    params: { dir: { kind: "enum", ask: "Split beside or below?", options: () => ({ right: "Beside", down: "Below" }) } },
    toCommands: (args) => [{ type: "pane.split", dir: args.dir === "down" ? "col" : "row" }],
    palette: () => [
      { title: "Split right", args: { dir: "right" } },
      { title: "Split down", args: { dir: "down" } },
    ],
  },
  {
    id: "close_pane",
    title: "Close pane",
    description: "Close a terminal pane and stop the agent running in it",
    risk: "safe",
    params: { target: { kind: "enum", ask: "Which pane should be closed?", options: paneTargets } },
    toCommands: (args) => [{ type: "pane.close", target: target(args) }],
    palette: () => [{ title: "Close pane", args: { target: "focused" } }],
  },
  {
    id: "focus_pane",
    title: "Focus pane",
    description: "Move keyboard focus to a specific pane",
    risk: "safe",
    params: { target: { kind: "enum", ask: "Which pane should get focus?", options: paneTargets } },
    toCommands: (args) => [{ type: "pane.focus", target: target(args) }],
    palette: (ctx) =>
      Object.entries(paneTargets(ctx))
        .filter(([id]) => id !== "focused")
        .map(([id, label]) => ({ title: `Focus ${label}`, args: { target: id } })),
  },
  {
    id: "focus_attention",
    title: "Next agent needing attention",
    description: "Jump to the next agent that is waiting for input or has finished",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "pane.nextAttention" }],
    palette: () => [{ title: "Next agent needing attention", args: {} }],
  },
  {
    id: "zoom_pane",
    title: "Zoom pane",
    description: "Show one pane full size, or restore the tiled layout",
    risk: "safe",
    params: { target: { kind: "enum", ask: "Which pane should be zoomed?", options: paneTargets } },
    toCommands: (args) => [{ type: "pane.zoom", target: target(args) }],
    palette: () => [{ title: "Toggle zoom", args: { target: "focused" } }],
  },
  {
    id: "resize_pane",
    title: "Resize pane",
    description: "Make a pane wider, narrower, taller or shorter",
    risk: "safe",
    params: {
      target: { kind: "enum", ask: "Which pane should be resized?", options: paneTargets },
      change: {
        kind: "enum",
        ask: "How should the pane change?",
        options: () => ({ wider: "Wider", narrower: "Narrower", taller: "Taller", shorter: "Shorter", bigger: "Bigger overall", smaller: "Smaller overall" }),
      },
      amount: { kind: "level", ask: "By how much?", levels: ["slightly", "moderately", "a lot"] },
    },
    toCommands: (args) => {
      const delta = RESIZE_STEP[String(args.amount)] ?? 0.05;
      const t = target(args);
      const width = (d: number): Command => ({ type: "pane.resize", target: t, axis: "row", delta: d });
      const height = (d: number): Command => ({ type: "pane.resize", target: t, axis: "col", delta: d });
      switch (args.change) {
        case "wider": return [width(delta)];
        case "narrower": return [width(-delta)];
        case "taller": return [height(delta)];
        case "shorter": return [height(-delta)];
        case "smaller": return [width(-delta), height(-delta)];
        default: return [width(delta), height(delta)];
      }
    },
    palette: () =>
      (["wider", "narrower", "taller", "shorter"] as const).map((change) => ({
        title: `Make pane ${change}`,
        args: { target: "focused", change, amount: "slightly" },
      })),
  },
  {
    id: "swap_panes",
    title: "Swap pane",
    description: "Swap the focused pane with its neighbor in a direction",
    risk: "safe",
    params: { dir: { kind: "enum", ask: "Swap in which direction?", options: () => ({ ...DIRECTIONS }) } },
    toCommands: (args) => [{ type: "pane.swapDir", dir: args.dir as keyof typeof DIRECTIONS }],
    palette: () => Object.entries(DIRECTIONS).map(([dir, label]) => ({ title: `Swap pane ${label.toLowerCase()}`, args: { dir } })),
  },
  {
    id: "show_changes",
    title: "Show changes",
    description: "Show the code changes an agent made in its worktree",
    risk: "safe",
    params: { target: { kind: "enum", ask: "Whose changes?", options: paneTargets } },
    toCommands: (args) => [{ type: "pane.showChanges", target: target(args) }],
    palette: () => [{ title: "Show changes", args: { target: "focused" } }],
  },
  {
    id: "merge_worktree",
    title: "Merge worktree",
    description: "Merge an agent's worktree branch back into the main branch",
    risk: "confirm",
    params: { target: { kind: "enum", ask: "Whose worktree?", options: paneTargets } },
    toCommands: (args) => [{ type: "pane.mergeWorktree", target: target(args) }],
    palette: () => [{ title: "Merge worktree into base branch", args: { target: "focused" } }],
  },
  {
    id: "discard_worktree",
    title: "Discard worktree",
    description: "Close an agent and delete its worktree and branch",
    risk: "confirm",
    params: { target: { kind: "enum", ask: "Whose worktree?", options: paneTargets } },
    toCommands: (args) => [{ type: "pane.discardWorktree", target: target(args) }],
    palette: () => [{ title: "Discard worktree", args: { target: "focused" } }],
  },
  {
    id: "toggle_worktrees",
    title: "Toggle worktrees",
    description: "Turn separate git worktrees for new agents on or off in this workspace",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "workspace.toggleWorktrees" }],
    palette: (ctx) => [{ title: `${ctx.active.useWorktrees ? "Disable" : "Enable"} worktrees for new agents`, args: {} }],
  },
  {
    id: "equalize_panes",
    title: "Equalize panes",
    description: "Give all panes in the layout equal size",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "layout.equalize" }],
    palette: () => [{ title: "Equalize panes", args: {} }],
  },
  {
    id: "set_layout",
    title: "Set layout",
    description: "Rearrange all panes into a grid, columns, rows or main-and-stack",
    risk: "safe",
    params: {
      preset: {
        kind: "enum",
        ask: "Which layout?",
        options: () => ({ auto: "Automatic spiral", "main-stack": "One big pane with the rest below", grid: "Grid", columns: "Side by side columns", rows: "Stacked rows" }),
      },
    },
    toCommands: (args) => [{ type: "layout.preset", name: args.preset as Preset }],
    palette: () => PRESETS.map((preset) => ({ title: `Layout: ${preset}`, args: { preset } })),
  },
  {
    id: "switch_workspace",
    title: "Switch workspace",
    description: "Show a different workspace and its panes",
    risk: "safe",
    params: {
      workspace: { kind: "enum", ask: "Which workspace?", options: (ctx) => Object.fromEntries(ctx.workspaces.map((w) => [w.id, w.name])) },
    },
    toCommands: (args) => [{ type: "workspace.switch", target: { id: String(args.workspace) } }],
    palette: (ctx) => ctx.workspaces.map((w) => ({ title: `Workspace: ${w.name}`, args: { workspace: w.id } })),
  },
  {
    id: "create_workspace",
    title: "New workspace",
    description: "Create a new workspace for a project folder",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "workspace.create" }],
    palette: () => [{ title: "New workspace…", args: {} }],
  },
  {
    id: "open_url",
    title: "Open URL",
    description: "Open a web address or local dev server in the built-in browser",
    risk: "safe",
    params: {
      url: { kind: "span", ask: "Which address or site should be opened?" },
      newTab: { kind: "bool", ask: "Did the user ask for a new tab?" },
    },
    toCommands: (args) => [{ type: "browser.open", url: String(args.url ?? ""), newTab: Boolean(args.newTab) }],
    palette: () => [],
  },
  {
    id: "browser_new_tab",
    title: "New browser tab",
    description: "Open an empty tab in the built-in browser",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "browser.newTab" }],
    palette: () => [{ title: "Browser: new tab", args: {} }],
  },
  {
    id: "browser_close_tab",
    title: "Close browser tab",
    description: "Close the current tab in the built-in browser",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "browser.closeTab" }],
    palette: () => [{ title: "Browser: close tab", args: {} }],
  },
  {
    id: "browser_navigate",
    title: "Browser back, forward or reload",
    description: "Go back, go forward or reload the page in the built-in browser",
    risk: "safe",
    params: { action: { kind: "enum", ask: "Back, forward or reload?", options: () => ({ back: "Back", forward: "Forward", reload: "Reload" }) } },
    toCommands: (args) => [{ type: "browser.history", delta: args.action === "back" ? -1 : args.action === "forward" ? 1 : 0 }],
    palette: () => [
      { title: "Browser: back", args: { action: "back" } },
      { title: "Browser: forward", args: { action: "forward" } },
      { title: "Browser: reload", args: { action: "reload" } },
    ],
  },
  {
    id: "set_device",
    title: "Browser device",
    description: "Preview the page as a phone or as a desktop browser",
    risk: "safe",
    params: { device: { kind: "enum", ask: "Phone or desktop?", options: () => ({ mobile: "Phone", desktop: "Desktop" }) } },
    toCommands: (args) => [{ type: "browser.device", device: args.device === "mobile" ? "mobile" : "desktop" }],
    palette: () => [
      { title: "Browser: mobile view", args: { device: "mobile" } },
      { title: "Browser: desktop view", args: { device: "desktop" } },
    ],
  },
  {
    id: "focus_address_bar",
    title: "Browser address bar",
    description: "Put the cursor in the browser address bar",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "browser.focusUrl" }],
    palette: () => [{ title: "Browser: open address…", args: {} }],
  },
  {
    id: "toggle_sidebar",
    title: "Toggle sidebar",
    description: "Show or hide the workspace sidebar",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "ui.toggleSidebar" }],
    palette: () => [{ title: "Toggle sidebar", args: {} }],
  },
  {
    id: "toggle_browser",
    title: "Toggle browser",
    description: "Show or hide the browser panel",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "ui.toggleBrowser" }],
    palette: () => [{ title: "Toggle browser", args: {} }],
  },
  {
    id: "show_shortcuts",
    title: "Keyboard shortcuts",
    description: "Show the list of keyboard shortcuts",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "ui.cheatSheet" }],
    palette: () => [{ title: "Keyboard shortcuts", args: {} }],
  },
  {
    id: "none",
    title: "Nothing",
    description: "Not a command, unclear, or background speech",
    risk: "safe",
    params: {},
    toCommands: () => [],
  },
];
