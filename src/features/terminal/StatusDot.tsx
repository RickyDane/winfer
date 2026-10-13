import { cn } from "@/lib/cn";
import type { AgentStatus } from "@/stores/workspaces";

const COLORS: Record<AgentStatus, string> = {
  idle: "bg-fg-subtle",
  working: "bg-success",
  "needs-input": "bg-warning",
  done: "bg-accent",
  error: "bg-danger",
};

export function StatusDot({ status, className }: { status: AgentStatus; className?: string }) {
  return (
    <span
      title={status}
      className={cn(
        "inline-block size-1.5 shrink-0 rounded-full",
        COLORS[status],
        status === "working" && "animate-pulse",
        className,
      )}
    />
  );
}
