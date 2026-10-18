import { Spinner } from "@/components/Spinner";
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
  // A working agent is a running background task: it gets the spinning dots, not a pulse.
  if (status === "working") {
    return (
      <span title={status} className={cn("-mx-0.5 inline-flex shrink-0", className)}>
        <Spinner size={10} className="text-success" />
      </span>
    );
  }
  return (
    <span
      title={status}
      className={cn(
        "inline-block size-1.5 shrink-0 rounded-full",
        COLORS[status],
        "transition-colors duration-300",
        status === "needs-input" && "anim-ring text-warning",
        className,
      )}
    />
  );
}
