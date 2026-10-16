/** Buttons plugins add to a side panel's toolbar (`api.registerPanelAction`). */
import { Puzzle } from "lucide-react";
import { cn } from "@/lib/cn";
import { PAGE_ICONS } from "./PluginPage";
import { runPanelAction, usePlugins, type ActionPanel } from "./plugins";

export function PanelActions({ panel }: { panel: ActionPanel }) {
  const actions = usePlugins((s) => s.panelActions).filter((a) => a.panel === panel);
  return actions.map((action) => {
    const Icon = PAGE_ICONS[action.icon ?? ""] ?? Puzzle;
    return (
      <button
        key={action.key}
        title={action.title}
        onClick={() => void runPanelAction(action)}
        className={cn(
          "flex shrink-0 items-center gap-1 rounded p-1 hover:bg-hover",
          action.active ? "text-danger" : "text-fg-subtle hover:text-fg",
        )}
      >
        <Icon size={14} className={cn(action.active && "animate-pulse")} />
        {action.label && <span className="font-mono text-[10px] tabular-nums">{action.label}</span>}
      </button>
    );
  });
}
