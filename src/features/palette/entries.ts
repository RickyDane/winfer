import { execute } from "@/commands/bus";
import { bindingFor, formatKeys } from "@/commands/keymap";
import { TOOLS, type ToolContext } from "@/tools/registry";
import { useWorkspaces } from "@/stores/workspaces";
import { useBrowser } from "@/features/browser/browser";

export interface Entry {
  id: string;
  title: string;
  keys?: string;
  run: () => void;
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
