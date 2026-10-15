import { Coins } from "lucide-react";
import { cn } from "@/lib/cn";
import { formatCost, formatTokens } from "./history";
import { useTaskCost, type DirUsage } from "./taskCost";

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
