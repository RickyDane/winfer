import { Coins } from "lucide-react";
import { useEffect } from "react";
import { cn } from "@/lib/cn";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { formatCost, formatTokens } from "./history";
import { useUsage, type PaneSpend } from "./usage";
import { refreshSessionCosts, refreshTaskCosts, useSessionCost, useTaskCost, type DirUsage } from "./taskCost";

/** "Opus 5.5 $1.20 · 1.4M tokens" lines, for a tooltip. */
export function costBreakdown(usage: DirUsage, where = "in this worktree"): string {
  const lines = usage.models.map((m) => `${m.model}: ${m.priced ? formatCost(m.cost) : "no known price"} · ${formatTokens(m.input + m.output + m.cacheRead + m.cacheWrite)} tokens`);
  return [`At API prices, ${usage.calls} model calls ${where}:`, ...lines].join("\n");
}

/** What the agents spent in a worktree so far; nothing until there's usage. */
export function TaskCost({ dir, className }: { dir: string | null | undefined; className?: string }) {
  return <CostChip usage={useTaskCost(dir)} className={className} />;
}

function CostChip({ usage, where, className }: { usage: DirUsage | undefined; where?: string; className?: string }) {
  if (!usage || usage.calls === 0) return null;
  return (
    <span className={cn("flex shrink-0 items-center gap-1 font-mono text-[11px] text-fg-muted", className)} title={costBreakdown(usage, where)}>
      <Coins size={11} className="text-fg-subtle" />
      {usage.priced || usage.cost > 0 ? formatCost(usage.cost) : "—"}
      <span className="text-fg-subtle">· {formatTokens(usage.tokens)}</span>
    </span>
  );
}

/**
 * A pane's task cost in its header, when turned on in Settings → Usage: its worktree's spend, or
 * for a pane sharing its folder with others, its agent session's. Reads fresh numbers when shown,
 * when the agent changes status and every 30s while it works.
 */
export function PaneCost({ pane }: { pane: Pane }) {
  const show = useWorkspaces((s) => s.showPaneCost);
  const reported = useUsage((s) => s.paneSpend[pane.id]);
  if (show && reported) return <ReportedCost spend={reported} />;
  return <LoggedCost pane={pane} show={show && pane.provider !== "winfer"} />;
}

/** What the agent itself says it spent this session (winfer code reports it after each turn). */
function ReportedCost({ spend }: { spend: PaneSpend }) {
  const tokens = spend.input + spend.output;
  if (tokens === 0) return null;
  const title = [
    "This session" + (spend.cost !== null ? ", at API prices:" : ":"),
    `${formatTokens(spend.input)} in (${formatTokens(spend.cached)} cached) · ${formatTokens(spend.output)} out`,
    spend.cost !== null ? `Cost: ${formatCost(spend.cost)} (your plan may cover it)` : "No price reported (billed to your plan)",
  ].join("\n");
  return (
    <span className="hidden @[24rem]:contents">
      <span className="flex shrink-0 items-center gap-1 font-mono text-[11px] text-fg-muted" title={title}>
        <Coins size={11} className="text-fg-subtle" />
        {spend.cost !== null ? formatCost(spend.cost) : "—"}
        <span className="text-fg-subtle">· {formatTokens(tokens)}</span>
      </span>
    </span>
  );
}

function LoggedCost({ pane, show }: { pane: Pane; show: boolean }) {
  const dir = show ? pane.worktree?.path : undefined;
  const session = show && !dir ? pane.sessionId : null;
  const status = pane.status;
  useEffect(() => {
    if (!dir && !session) return;
    const refresh = () => void (dir ? refreshTaskCosts([dir]) : session && refreshSessionCosts([session]));
    refresh();
    if (status !== "working") return;
    const timer = window.setInterval(refresh, 30_000);
    return () => window.clearInterval(timer);
  }, [dir, session, status]);
  const dirUsage = useTaskCost(dir);
  const sessionUsage = useSessionCost(session);
  if (!dir && !session) return null;
  // Too narrow a header keeps its room for the title.
  return (
    <span className="hidden @[24rem]:contents">
      <CostChip usage={dir ? dirUsage : sessionUsage} where={dir ? undefined : "in this session"} />
    </span>
  );
}
