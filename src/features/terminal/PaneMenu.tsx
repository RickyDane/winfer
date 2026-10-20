import { Ellipsis } from "lucide-react";
import { useRef, useState } from "react";
import { createPortal } from "react-dom";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { execute, type Command } from "@/commands/bus";
import { ContextMenu, type MenuItem } from "@/components/ContextMenu";
import { preferredEditor } from "@/features/editors/editors";
import type { Pane } from "@/stores/workspaces";

/** Width of ContextMenu (w-60), to right-align it under the button. */
const MENU_WIDTH = 240;

/** Per-pane actions behind the "…" button. Portaled to <body> so the pane's overflow clipping can't cut it off. */
export function PaneMenu({ pane, onRename }: { pane: Pane; onRename: () => void }) {
  const [at, setAt] = useState<{ x: number; y: number } | null>(null);
  const button = useRef<HTMLButtonElement>(null);

  const toggle = () => {
    if (at) return setAt(null);
    const r = button.current?.getBoundingClientRect();
    if (r) setAt({ x: r.right - MENU_WIDTH, y: r.bottom + 4 });
  };

  const target = { id: pane.id };
  const editor = preferredEditor();
  const items: MenuItem[] = [
    { label: "Rename…", run: onRename },
    ...(pane.worktree
      ? [
          { label: "Show changes", run: () => execute({ type: "pane.showChanges", target }) },
          { label: `Merge into ${pane.worktree.base}`, run: () => execute({ type: "pane.mergeWorktree", target }) },
          { label: "Reveal worktree in Finder", run: () => void revealItemInDir(pane.worktree!.path) },
          { label: "Discard worktree", run: () => execute({ type: "pane.discardWorktree", target }), danger: true },
        ]
      : []),
    ...(editor ? [{ label: `Open in ${editor.name}`, run: () => execute({ type: "editor.open", target }) }] : []),
    {
      label: "Change model…",
      run: () => {
        execute({ type: "pane.focus", target });
        execute({ type: "ui.overlay", overlay: "model" });
      },
    },
    ...(pane.archived
      ? [{ label: "Restore from archive", run: () => execute({ type: "pane.archive", target, restore: true }) }]
      : [
          { label: "Zoom", run: () => execute({ type: "pane.zoom", target } satisfies Command) },
          { label: "Archive", run: () => execute({ type: "pane.archive", target }) },
        ]),
    { label: "Close", run: () => execute({ type: "pane.close", target }) },
  ];

  return (
    <div className="relative" onMouseDown={(e) => e.stopPropagation()}>
      <button ref={button} title="More" onClick={toggle} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
        <Ellipsis size={14} />
      </button>
      {at && createPortal(<ContextMenu x={at.x} y={at.y} items={items} onClose={() => setAt(null)} />, document.body)}
    </div>
  );
}
