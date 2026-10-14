import { Check, Layers, Square } from "lucide-react";
import { useState } from "react";
import { execute } from "@/commands/bus";
import type { AgentStatus, Pane } from "@/stores/workspaces";

type Target = "all" | "working" | "needs-input";

const TARGETS: { id: Target; label: string }[] = [
  { id: "all", label: "All agents" },
  { id: "working", label: "Working" },
  { id: "needs-input", label: "Waiting for me" },
];

const isTarget = (target: Target, status: AgentStatus) => target === "all" || status === target;

/**
 * Messages, approves or interrupts a group of agents at once: every agent of the active
 * workspace, or (in the overview) of every workspace.
 */
export function AgentBar({ panes, scope }: { panes: Pane[]; scope: { workspace: string } | { workspaces: number } }) {
  const [target, setTarget] = useState<Target>("all");
  const [text, setText] = useState("");
  const agents = panes.filter((p) => p.provider !== "shell" && isTarget(target, p.status));
  const waiting = panes.filter((p) => p.status === "needs-input").length;

  const send = (submit: boolean) => {
    if (!text.trim() || agents.length === 0) return;
    execute({ type: "pane.broadcast", text: text.trim(), submit, paneIds: agents.map((p) => p.id) });
    setText("");
  };
  const each = (type: "pane.interrupt" | "pane.approve") => agents.forEach((p) => execute({ type, target: { id: p.id } }));

  return (
    <div className="flex shrink-0 flex-wrap items-center gap-2 pb-2 text-xs">
      <span className="flex shrink-0 items-center gap-1.5 text-fg-muted">
        <Layers size={13} className="text-accent" />
        {panes.length} session{panes.length === 1 ? "" : "s"}
        {"workspaces" in scope && ` in ${scope.workspaces} workspace${scope.workspaces === 1 ? "" : "s"}`}
        {waiting > 0 && <span className="text-warning">· {waiting} waiting</span>}
      </span>
      <select
        value={target}
        onChange={(e) => setTarget(e.target.value as Target)}
        className="h-7 shrink-0 rounded-md border border-border bg-panel px-1.5 text-xs text-fg"
        title="Which agents the message and buttons reach"
      >
        {TARGETS.map((t) => (
          <option key={t.id} value={t.id}>
            {t.label} ({panes.filter((p) => p.provider !== "shell" && isTarget(t.id, p.status)).length})
          </option>
        ))}
      </select>
      <form
        className="flex min-w-56 flex-1 items-center gap-1 rounded-md border border-border bg-panel px-2 focus-within:border-accent"
        onSubmit={(e) => {
          e.preventDefault();
          send(true);
        }}
      >
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={`Message ${agents.length} agent${agents.length === 1 ? "" : "s"} ${"workspaces" in scope ? "across workspaces" : `in ${scope.workspace}`} (Enter sends)`}
          className="h-7 min-w-0 flex-1 bg-transparent text-xs text-fg outline-none placeholder:text-fg-subtle"
        />
        <button type="button" onClick={() => send(false)} disabled={!text.trim()} className="text-[11px] whitespace-nowrap text-fg-subtle hover:text-fg disabled:opacity-40">
          Type only
        </button>
      </form>
      <button
        onClick={() => each("pane.approve")}
        disabled={agents.length === 0}
        className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-fg-muted hover:bg-hover hover:text-fg disabled:opacity-40"
        title="Accept the pending request of these agents"
      >
        <Check size={12} /> Approve
      </button>
      <button
        onClick={() => each("pane.interrupt")}
        disabled={agents.length === 0}
        className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-fg-muted hover:bg-hover hover:text-fg disabled:opacity-40"
        title="Stop what these agents are doing (Esc)"
      >
        <Square size={10} fill="currentColor" /> Interrupt
      </button>
    </div>
  );
}
