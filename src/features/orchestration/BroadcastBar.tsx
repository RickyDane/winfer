import { Send } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { execute } from "@/commands/bus";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { leaves } from "@/features/layout/tree";
import { cn } from "@/lib/cn";
import { providerName, useActiveWorkspace, useWorkspaces } from "@/stores/workspaces";

/** ⌘⇧I: type once, send to several agents. */
export function BroadcastBar() {
  const { overlay, setOverlay, providers } = useWorkspaces();
  const workspace = useActiveWorkspace();
  const agents = workspace?.layout
    ? leaves(workspace.layout).map((id) => workspace.panes.find((p) => p.id === id)!).filter((p) => p.provider !== "shell")
    : [];
  const [text, setText] = useState("");
  const [excluded, setExcluded] = useState<Set<string>>(new Set());
  const input = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (overlay === "broadcast") input.current?.focus();
  }, [overlay]);

  if (overlay !== "broadcast" || !workspace) return null;
  const targets = agents.filter((p) => !excluded.has(p.id));
  const send = () => {
    if (!text.trim() || targets.length === 0) return;
    execute({ type: "pane.broadcast", text: text.trim(), paneIds: targets.map((p) => p.id) });
    setText("");
    setOverlay(null);
  };

  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex justify-center bg-black/40 pt-[18vh]" onMouseDown={() => setOverlay(null)}>
      <div className="anim-dialog h-fit w-[600px] space-y-3 rounded-xl border border-border-strong bg-panel-raised p-4 shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <div className="text-xs font-medium tracking-wide text-fg-subtle uppercase">Send to agents</div>
        <textarea
          ref={input}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            } else if (e.key === "Escape") setOverlay(null);
          }}
          rows={3}
          placeholder="e.g. run the tests and fix any failures (⇧↩ for a new line)"
          className="w-full resize-none rounded-md border border-border bg-panel p-2.5 font-mono text-xs text-fg outline-none focus:border-accent"
        />
        <div className="flex flex-wrap items-center gap-1.5">
          {agents.length === 0 && <span className="text-xs text-fg-subtle">No agents in this workspace.</span>}
          {agents.map((p, i) => {
            const on = !excluded.has(p.id);
            return (
              <button
                key={p.id}
                onClick={() =>
                  setExcluded((prev) => {
                    const next = new Set(prev);
                    if (on) next.add(p.id);
                    else next.delete(p.id);
                    return next;
                  })
                }
                className={cn(
                  "flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs",
                  on ? "border-accent/60 bg-accent-soft text-fg" : "border-border text-fg-subtle line-through",
                )}
              >
                <ProviderIcon provider={p.provider} size={12} />
                {i + 1} · {providerName(providers, p.provider)}
              </button>
            );
          })}
          <button
            onClick={send}
            disabled={!text.trim() || targets.length === 0}
            className="ml-auto flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-white hover:opacity-90 disabled:opacity-40"
          >
            <Send size={12} /> Send to {targets.length}
          </button>
        </div>
      </div>
    </div>
  );
}
