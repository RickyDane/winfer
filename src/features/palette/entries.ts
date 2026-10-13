import { execute } from "@/commands/bus";
import { bindingFor, formatKeys } from "@/commands/keymap";
import { TOOLS, type ToolContext } from "@/tools/registry";
import { providerName, useWorkspaces } from "@/stores/workspaces";
import { leaves } from "@/features/layout/tree";
import { findAddresses } from "@/features/browser/addresses";
import { readTerminal } from "@/features/terminal/xtermRegistry";
import { getHome } from "@/lib/home";
import { useBrowser } from "@/features/browser/browser";

export interface Entry {
  id: string;
  title: string;
  keys?: string;
  run: () => void;
}

/** Option text that says what an address is, so "the page you made" vs "the dev server" is clear. */
export function describeAddress(url: string, where: string, newest: boolean): string {
  const kind = url.startsWith("file://")
    ? `HTML file ${decodeURI(url.slice("file://".length)).split("/").at(-1)} that ${where} created (${decodeURI(url.slice("file://".length))})`
    : /localhost|127\.0\.0\.1/.test(url)
      ? `local dev server at ${url.replace(/^https?:\/\//, "")} started in ${where}`
      : `web page ${url} mentioned in ${where}`;
  return newest ? `${kind}, most recent` : kind;
}

/** Addresses agents mentioned in the active workspace, focused pane first, newest first. */
function recentAddresses(): Record<string, string> {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId);
  if (!ws?.layout) return {};
  const order = leaves(ws.layout);
  const panes = [...order].sort((a, b) => Number(b === s.focusedPaneId) - Number(a === s.focusedPaneId));
  const out: Record<string, string> = {};
  for (const id of panes) {
    const pane = ws.panes.find((p) => p.id === id)!;
    const where = `pane ${order.indexOf(id) + 1} (${providerName(s.providers, pane.provider)})`;
    const found = findAddresses(readTerminal(id, 400) ?? "", pane.cwd, getHome());
    if (pane.devUrl) found.unshift(pane.devUrl);
    for (const url of found) {
      if (out[url] || Object.keys(out).length >= 12) continue;
      out[url] = describeAddress(url, where, Object.keys(out).length === 0);
    }
  }
  return out;
}

export function toolContext(lastCommands: ToolContext["lastCommands"] = []): ToolContext {
  const s = useWorkspaces.getState();
  const tabs = useBrowser.getState().tabs[s.activeWorkspaceId] ?? [];
  return {
    providers: s.providers,
    workspaces: s.workspaces,
    active: s.workspaces.find((w) => w.id === s.activeWorkspaceId)!,
    focusedPaneId: s.focusedPaneId,
    browserTabs: tabs.map((t) => t.title || t.url),
    lastCommands,
    models: s.models,
    recentAddresses: recentAddresses(),
  };
}

export function paletteEntries(): Entry[] {
  const ctx = toolContext();
  return TOOLS.flatMap((tool) =>
    (tool.palette?.(ctx) ?? []).map((entry, i) => {
      const commands = tool.toCommands(entry.args, ctx);
      const binding = commands.length === 1 ? bindingFor(commands[0]) : undefined;
      return {
        id: `${tool.id}:${i}`,
        title: entry.title,
        keys: binding && formatKeys(binding),
        run: () => commands.forEach(execute),
      };
    }),
  );
}

/** Subsequence match; lower is better, null if no match. Rewards contiguous and word-start hits. */
export function score(query: string, text: string): number | null {
  const q = query.toLowerCase().trim();
  if (!q) return 0;
  const t = text.toLowerCase();
  let pos = -1;
  let total = 0;
  for (const ch of q) {
    const next = t.indexOf(ch, pos + 1);
    if (next === -1) return null;
    const gap = next - pos - 1;
    const wordStart = next === 0 || " :".includes(t[next - 1]);
    total += wordStart ? 0 : gap > 0 ? 2 + gap * 0.1 : 0;
    pos = next;
  }
  return total + t.length * 0.01;
}
