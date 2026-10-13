import { Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { useWorkspaces } from "@/stores/workspaces";

/** Provider picker for opening a new pane. M2 replaces this with ⌘T / ⌘⇧T and the palette. */
export function NewPaneMenu() {
  const { providers, addPane } = useWorkspaces();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);

  return (
    <div ref={root} className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 rounded-md border border-border px-2 py-1 text-xs hover:bg-hover hover:text-fg"
        title="New pane"
      >
        <Plus size={13} />
        <span>New</span>
      </button>
      {open && (
        <div className="absolute right-0 z-50 mt-1 w-56 rounded-lg border border-border-strong bg-panel-raised p-1 shadow-xl">
          {providers.map((p) => (
            <button
              key={p.id}
              disabled={!p.installed}
              onClick={() => {
                addPane(p.id);
                setOpen(false);
              }}
              className={cn(
                "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px]",
                p.installed ? "text-fg hover:bg-hover" : "cursor-not-allowed text-fg-subtle",
              )}
            >
              <ProviderIcon provider={p.id} />
              <span className="flex-1">{p.name}</span>
              {!p.installed && <span className="text-[11px]">not installed</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
