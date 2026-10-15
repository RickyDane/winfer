import { useEffect, useRef } from "react";
import { cn } from "@/lib/cn";
import { paneTitle, useWorkspaces, type Pane } from "@/stores/workspaces";

/**
 * Inline editor for a session's name. Enter or leaving the field saves, Escape cancels;
 * saving an empty name goes back to the agent's own title.
 */
export function PaneTitleInput({ pane, onDone, className }: { pane: Pane; onDone: () => void; className?: string }) {
  const input = useRef<HTMLInputElement>(null);
  const finished = useRef(false);

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);

  const finish = (save: boolean) => {
    if (finished.current) return;
    finished.current = true;
    if (save && input.current) useWorkspaces.getState().renamePane(pane.id, input.current.value);
    onDone();
  };

  return (
    <input
      ref={input}
      defaultValue={paneTitle(pane) ?? ""}
      placeholder="Name this session"
      aria-label="Session name"
      spellCheck={false}
      onBlur={() => finish(true)}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(true);
        else if (e.key === "Escape") finish(false);
      }}
      // Keeps clicks in the field from focusing, dragging or reordering the pane.
      onPointerDown={(e) => e.stopPropagation()}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
      className={cn("min-w-0 rounded border border-accent/70 bg-panel px-1 text-fg outline-none", className)}
    />
  );
}
