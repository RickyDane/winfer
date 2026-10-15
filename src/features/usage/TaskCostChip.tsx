import { Coins } from "lucide-react";
import { useEffect } from "react";
import { cn } from "@/lib/cn";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { formatCost, formatTokens } from "./history";
import { refreshTaskCosts, useTaskCost, type DirUsage } from "./taskCost";

/** "Opus 5.5 $1.20 · 1.4M tokens" lines, for a tooltip. */
export function costBreakdown(usage: DirUsage): string {
  const lines = usage.models.map((m) => `${m.model}: ${m.priced ? formatCost(m.cost) : "no known price"} · ${formatTokens(m.input + m.output + m.cacheRead + m.cacheWrite)} tokens`);
  return [`At API prices, ${usage.calls} model calls in this worktree:`, ...lines].join("\n");
}

/** What the agents spent in a worktree so far; nothing until there's usage. */
export function TaskCost({ dir, className }: { dir: string | null | undefined; className?: string }) {
  const usage = useTaskCost(dir);
  if (!usage || usage.calls === 0) return null;
  return (
    <span className={cn("flex shrink-0 items-center gap-1 font-mono text-[11px] text-fg-muted", className)} title={costBreakdown(usage)}>
      <Coins size={11} className="text-fg-subtle" />
      {usage.priced || usage.cost > 0 ? formatCost(usage.cost) : "—"}
      <span className="text-fg-subtle">· {formatTokens(usage.tokens)}</span>
    </span>
  );
}

/**
 * A worktree pane's task cost in its header, when turned on in Settings → Usage. Reads fresh
 * numbers when shown, when the agent changes status and every 30s while it works.
 */
export function PaneCost({ pane }: { pane: Pane }) {
  const show = useWorkspaces((s) => s.showPaneCost);
  const dir = show ? pane.worktree?.path : undefined;
  const status = pane.status;
  useEffect(() => {
    if (!dir) return;
    void refreshTaskCosts([dir]);
    if (status !== "working") return;
    const timer = window.setInterval(() => void refreshTaskCosts([dir]), 30_000);
    return () => window.clearInterval(timer);
  }, [dir, status]);
  if (!dir) return null;
  // Too narrow a header keeps its room for the title.
  return (
    <span className="hidden @[24rem]:contents">
      <TaskCost dir={dir} />
    </span>
  );
}
