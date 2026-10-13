import { Ellipsis } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { execute, type Command } from "@/commands/bus";
import type { Pane } from "@/stores/workspaces";

/** Per-pane actions behind the "…" button. */
export function PaneMenu({ pane }: { pane: Pane }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && setOpen(false);
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);

  const target = { id: pane.id };
  const items: { label: string; run: () => void; danger?: boolean }[] = [
    ...(pane.worktree
      ? [
          { label: "Show changes", run: () => execute({ type: "pane.showChanges", target }) },
          { label: `Merge into ${pane.worktree.base}`, run: () => execute({ type: "pane.mergeWorktree", target }) },
          { label: "Reveal worktree in Finder", run: () => void revealItemInDir(pane.worktree!.path) },
          { label: "Discard worktree", run: () => execute({ type: "pane.discardWorktree", target }), danger: true },
        ]
      : []),
    {
      label: "Change model…",
      run: () => {
        execute({ type: "pane.focus", target });
        execute({ type: "ui.overlay", overlay: "model" });
      },
    },
    { label: "Zoom", run: () => execute({ type: "pane.zoom", target } satisfies Command) },
    { label: "Close", run: () => execute({ type: "pane.close", target }) },
  ];

  return (
    <div ref={root} className="relative" onMouseDown={(e) => e.stopPropagation()}>
      <button title="More" onClick={() => setOpen((o) => !o)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
        <Ellipsis size={14} />
      </button>
      {open && (
        <div className="absolute right-0 z-40 mt-1 w-56 rounded-lg border border-border-strong bg-panel-raised p-1 shadow-xl">
          {items.map((item) => (
            <button
              key={item.label}
              onClick={() => {
                setOpen(false);
                item.run();
              }}
              className={`block w-full rounded-md px-2 py-1.5 text-left text-[13px] hover:bg-hover ${item.danger ? "text-danger" : "text-fg"}`}
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
