/**
 * Tool Registry: every user-facing action, described once.
 *
 * - `description` is what the Clef decision model sees as the option text (keep ~10–15 words).
 * - `params` map to System One question types (choice / noul / score) when voice lands in M5.
 * - `palette` expands a tool into concrete ⌘K entries.
 * - `toCommands` turns resolved arguments into Command Bus commands.
 */
import type { Command, PaneRef } from "@/commands/bus";
import type { ModelInfo, Provider } from "@/features/providers/providers";
import { leaves, PRESETS, type Preset } from "@/features/layout/tree";
import { providerName, type Workspace } from "@/stores/workspaces";
import { allRecipes, describeRecipe } from "@/features/recipes/recipes";

export interface ToolContext {
  providers: Provider[];
  workspaces: Workspace[];
  active: Workspace;
  focusedPaneId: string | null;
  /** Titles or URLs of the active workspace's browser tabs. */
  browserTabs: string[];
  /** Commands from the last executed voice request ("do that again"). */
  lastCommands: Command[];
  /** Models each provider offers. */
  models: Record<string, ModelInfo[]>;
  /** Web addresses and .html files recently mentioned in pane output (url → where from). */
  recentAddresses: Record<string, string>;
}

type Options = (ctx: ToolContext) => Record<string, string | null>;

export type ParamDef =
  | { kind: "enum"; ask: string; options: Options } // → choice
  | { kind: "count"; ask: string; max: number } // → choice "zero".."max"
  | { kind: "perOption"; ask: string; options: Options; max: number } // → one count question per option
  | { kind: "bool"; ask: string } // → noul
  | { kind: "level"; ask: string; levels: string[] } // → score
  /** Free text picked from the transcript. Tools share one question per slot. */
  | { kind: "span"; slot: "instruction" | "address"; ask: string }; // → choice over transcript spans

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

const installedAgents: Options = (ctx) =>
  Object.fromEntries(ctx.providers.filter((p) => p.installed && p.id !== "shell").map((p) => [p.id, p.name]));

/** Every known model across installed agents, for voice ("open Claude with Opus"). */
const modelOptions: Options = (ctx) => {
  const options: Record<string, string> = { default: "No model named; use the agent's default" };
  for (const p of ctx.providers.filter((p) => p.installed && p.modelArg)) {
    for (const m of (ctx.models[p.id] ?? []).slice(0, 40)) {
      if (!options[m.id] && Object.keys(options).length < 250) options[m.id] = `${m.label} (${p.name})`;
    }
  }
  return options;
};

/** A spoken model only applies to providers that offer it (or that can't list models). */
const modelFor = (ctx: ToolContext, provider: string, model: unknown): string | undefined => {
  if (!model || model === "default") return undefined;
  const list = ctx.models[provider] ?? [];
  return list.length === 0 || list.some((m) => m.id === model) ? String(model) : undefined;
};

/** Installed providers, with the names speech recognition tends to mishear them as. */
const installedProviders: Options = (ctx) =>
  Object.fromEntries(
    ctx.providers
      .filter((p) => p.installed)
      .map((p) => {
        const heard = p.aliases.filter((a) => a.toLowerCase() !== p.name.toLowerCase() && a !== p.id);
        return [p.id, heard.length ? `${p.name} (also heard as ${heard.join(", ")})` : p.name];
      }),
  );

/** Pane targets: "focused" plus every pane by id, described by number and provider. */
const paneTargets: Options = (ctx) => ({
  focused: "The pane that currently has focus",
  all: "All panes / every agent at once",
  ...Object.fromEntries(
    (ctx.active.layout ? leaves(ctx.active.layout) : []).map((id, i) => {
      const pane = ctx.active.panes.find((p) => p.id === id)!;
      return [id, `Pane ${i + 1}: ${providerName(ctx.providers, pane.provider)}, ${pane.status}`];
    }),
  ),
});

const target = (args: Args): PaneRef =>
  args.target && args.target !== "focused" && args.target !== "all" ? { id: String(args.target) } : "focused";

/** Panes a command applies to: every pane for "all", else the one target. */
const targets = (args: Args, ctx: ToolContext): PaneRef[] =>
  args.target === "all" ? (ctx.active.layout ? leaves(ctx.active.layout) : []).map((id) => ({ id })) : [target(args)];

/** Runs `make` once per targeted pane. */
const perPane = (args: Args, ctx: ToolContext, make: (t: PaneRef) => Command | null): Command[] =>
  targets(args, ctx).flatMap((t) => {
    const cmd = make(t);
    return cmd ? [cmd] : [];
  });

const RESIZE_STEP: Record<string, number> = { slightly: 0.05, moderately: 0.1, "a lot": 0.2 };
const DIRECTIONS = { left: "Left", right: "Right", up: "Up", down: "Down" } as const;

export const TOOLS: ToolDef[] = [
  {
    id: "spawn_panes",
    title: "New pane",
    description: "Open, start, launch or spin up new agents, terminals or windows (Claude Code, Codex, …), one or several, optionally with a specific model or in their own worktrees. Names may be misheard: cloud means Claude, codecs means Codex",
    risk: "safe",
    params: {
      counts: { kind: "perOption", ask: "How many new panes of this provider are requested?", options: installedProviders, max: 9 },
      total: { kind: "count", ask: "How many new panes or windows are requested in total?", max: 9 },
      prompt: { kind: "span", slot: "instruction", ask: "What should the new agent be told to do, if anything (e.g. “create a Flappy Bird clone”)?" },
      model: { kind: "enum", ask: "Which AI model did the user name, e.g. Opus or Sonnet? default if none.", options: modelOptions },
    },
    toCommands: (args, ctx) => {
      const prompt = args.prompt ? String(args.prompt) : undefined;
      const named = Object.entries((args.counts as Record<string, number>) ?? {}).filter(([, n]) => n > 0);
      // "Open five windows" names no agent: open the default agent that many times.
      if (named.length === 0) {
        const count = Math.max(1, Number(args.total) || 1);
        return [{ type: "pane.spawn", provider: "default", count, prompt }];
      }
      return named.map(([provider, count]) => ({ type: "pane.spawn", provider, count, prompt, model: modelFor(ctx, provider, args.model) }));
    },
    palette: (ctx) =>
      ctx.providers
        .filter((p) => p.installed)
        .map((p) => ({ title: `New pane: ${p.name}`, args: { counts: { [p.id]: 1 } } })),
  },
  {
    id: "split_pane",
    title: "Split pane",
    description: "Split the focused pane in two; only when the user literally says split",
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
    toCommands: (args) => (args.target === "all" ? [{ type: "pane.closeAll" }] : [{ type: "pane.close", target: target(args) }]),
    palette: () => [{ title: "Close pane", args: { target: "focused" } }],
  },
  {
    id: "change_model",
    title: "Change model",
    description: "Change or switch the model of an agent that is already running, when the user says switch or change, e.g. switch Claude to Opus. Not when the user says open, start or create",
    risk: "safe",
    params: {
      target: { kind: "enum", ask: "Which pane does the request refer to?", options: paneTargets },
      model: { kind: "enum", ask: "Which AI model did the user name, e.g. Opus or Sonnet? default if none.", options: modelOptions },
    },
    toCommands: (args, ctx) => {
      if (!args.model || args.model === "default") return [{ type: "ui.overlay", overlay: "model" }];
      // Each pane gets the model only if its agent offers it ("change all the models to Opus").
      const commands = perPane(args, ctx, (t) => {
        const id = t === "focused" ? ctx.focusedPaneId : "id" in t ? t.id : null;
        const pane = ctx.active.panes.find((p) => p.id === id);
        const model = pane && modelFor(ctx, pane.provider, args.model);
        return model ? { type: "pane.setModel", target: t, model } : null;
      });
      return commands.length ? commands : [{ type: "ui.overlay", overlay: "model" }];
    },
    palette: () => [{ title: "Change model…", args: {} }],
  },
  {
    id: "close_all_panes",
    title: "Close all panes",
    description: "Close all panes, windows or agents in this workspace at once",
    risk: "confirm",
    params: {},
    toCommands: () => [{ type: "pane.closeAll" }],
    palette: () => [{ title: "Close all panes", args: {} }],
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
    id: "review_work",
    title: "Review finished work",
    description: "Show what finished agents did, ready to merge; answers “what's done?”, “anything to review?”",
    risk: "safe",
    params: { read_aloud: { kind: "bool", ask: "Did the user ask a question to be answered out loud, like “what's done?”" } },
    toCommands: (args) => [{ type: "ui.overlay", overlay: "review" }, ...(args.read_aloud ? [{ type: "review.readAloud" } as const] : [])],
    palette: () => [
      { title: "Review finished work", args: {} },
      { title: "Read finished work aloud", args: { read_aloud: true } },
    ],
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
    description: "Change the workspace setting that turns worktrees on or off; not for opening agents",
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
    description: "Open a website, web address, localhost dev server or a page an agent created in the built-in browser",
    risk: "safe",
    params: {
      url: { kind: "span", slot: "address", ask: "Which address or site should be opened?" },
      recent: {
        kind: "enum",
        ask: "Which of the addresses the agents recently mentioned does the user mean (e.g. “the page you created”)? none if the user said an address.",
        options: (ctx) => ({ none: "None of these / the user said an address", ...ctx.recentAddresses }),
      },
      newTab: { kind: "bool", ask: "Did the user ask for a new tab?" },
    },
    toCommands: (args, ctx) => {
      const recent = args.recent && args.recent !== "none" ? String(args.recent) : undefined;
      // Only words that look like an address count ("github.com", "localhost 3000"), not "the page".
      const spoken = typeof args.url === "string" && /(\.[a-z]{2,}\b|localhost|\b\d{2,5}\b|\/)/i.test(args.url) ? args.url.trim() : undefined;
      // A spoken address wins unless it's just words ("the page") and an agent's address fits.
      const url = recent ?? spoken ?? Object.keys(ctx.recentAddresses)[0];
      return url ? [{ type: "browser.open", url, newTab: Boolean(args.newTab) }] : [{ type: "browser.focusUrl" }];
    },
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
    id: "pick_element",
    title: "Pick element",
    description: "Pick an element on the web page to show the agent what to change",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "browser.pick" }],
    palette: () => [{ title: "Browser: pick element for agent", args: {} }],
  },
  {
    id: "send_console_errors",
    title: "Send console errors",
    description: "Send the web page's console and network errors to the agent",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "browser.sendConsole" }],
    palette: () => [{ title: "Browser: send console errors to agent", args: {} }],
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
    id: "open_settings",
    title: "Settings",
    description: "Open winfer's settings",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "ui.settings" }],
    palette: () => [{ title: "Settings", args: {} }],
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
    id: "send_to_pane",
    title: "Tell an agent",
    description: "Tell an existing agent to do something, or type or insert text into a pane, e.g. tell Claude to run the tests",
    risk: "safe",
    params: {
      target: { kind: "enum", ask: "Which pane does the request refer to?", options: paneTargets },
      text: { kind: "span", slot: "instruction", ask: "What exactly should be typed to the agent?" },
      submit: { kind: "bool", ask: "Should the message be sent right away? No if the user only said type or insert it." },
    },
    toCommands: (args, ctx) =>
      args.text ? perPane(args, ctx, (t) => ({ type: "pane.sendText", target: t, text: String(args.text), submit: args.submit !== false })) : [],
  },
  {
    id: "interrupt_agent",
    title: "Interrupt agent",
    description: "Stop or interrupt what an agent is currently doing",
    risk: "safe",
    params: { target: { kind: "enum", ask: "Which pane does the request refer to?", options: paneTargets } },
    toCommands: (args, ctx) => (args.target === "all" ? [{ type: "pane.interruptAll" }] : perPane(args, ctx, (t) => ({ type: "pane.interrupt", target: t }))),
    palette: () => [{ title: "Interrupt agent", args: { target: "focused" } }],
  },
  {
    id: "interrupt_all",
    title: "Stop all agents",
    description: "Stop or interrupt all agents and sessions at once, keeping their panes open",
    risk: "safe",
    params: {},
    toCommands: () => [{ type: "pane.interruptAll" }],
    palette: () => [{ title: "Stop all agents", args: {} }],
  },
  {
    id: "approve_agent",
    title: "Approve agent",
    description: "Approve or allow the permission an agent is asking for",
    risk: "confirm",
    params: { target: { kind: "enum", ask: "Which pane does the request refer to?", options: paneTargets } },
    toCommands: (args, ctx) => perPane(args, ctx, (t) => ({ type: "pane.approve", target: t })),
    palette: () => [{ title: "Approve agent's request", args: { target: "focused" } }],
  },
  {
    id: "broadcast",
    title: "Tell all agents",
    description: "Tell several or all agents that are already open to do something, possibly different things each (e.g. “all build X except the last one, which builds Y”). Not for opening or creating new windows",
    risk: "safe",
    params: { text: { kind: "span", slot: "instruction", ask: "What exactly should be sent to all agents?" } },
    toCommands: (args) => (args.text ? [{ type: "pane.broadcast", text: String(args.text) }] : [{ type: "ui.overlay", overlay: "broadcast" }]),
    palette: () => [{ title: "Broadcast to all agents…", args: {} }],
  },
  {
    id: "race_agents",
    title: "Race agents",
    description: "Start new agents that race each other on the same task to compare who does it best; only when the user says race, compete or compare",
    risk: "safe",
    params: {
      counts: { kind: "perOption", ask: "How many agents of this provider should race?", options: installedAgents, max: 3 },
      prompt: { kind: "span", slot: "instruction", ask: "What task should the agents race on?" },
    },
    toCommands: (args) => {
      const counts = (args.counts as Record<string, number>) ?? {};
      return args.prompt && Object.values(counts).some((n) => n > 0)
        ? [{ type: "race.start", counts, prompt: String(args.prompt) }]
        : [{ type: "ui.overlay", overlay: "race" }];
    },
    palette: () => [{ title: "Race mode…", args: {} }],
  },
  {
    id: "load_recipe",
    title: "Load recipe",
    description: "Set up a saved recipe of agents, layout and preview, like pair programming or full stack",
    risk: "safe",
    params: { recipe: { kind: "enum", ask: "Which recipe?", options: () => Object.fromEntries(allRecipes().map((r) => [r.id, `${r.name}: ${describeRecipe(r)}`])) } },
    toCommands: (args) => (args.recipe ? [{ type: "recipe.load", id: String(args.recipe) }] : [{ type: "ui.overlay", overlay: "recipes" }]),
    palette: () => [...allRecipes().map((r) => ({ title: `Recipe: ${r.name}`, args: { recipe: r.id } })), { title: "Recipes…", args: {} }],
  },
  {
    id: "repeat_last",
    title: "Repeat",
    description: "Do the previous voice command again",
    risk: "safe",
    params: {},
    toCommands: (_args, ctx) => ctx.lastCommands,
  },
  {
    id: "none",
    title: "Nothing",
    description: "Not a request to winfer: questions, chit-chat, weather, time, or unrelated speech",
    risk: "safe",
    params: {},
    toCommands: () => [],
  },
];
