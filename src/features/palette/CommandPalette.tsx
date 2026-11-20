import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowRight, Check, Cpu, FolderOpen, Globe, LayoutGrid, Palette as PaletteIcon, PanelRight, Plus, Search, Settings, X, Zap, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/cn";
import { useWorkspaces, useWorkspacesPick } from "@/stores/workspaces";
import { focusTerminal } from "@/features/terminal/xtermRegistry";
import { paletteEntries, score } from "./entries";

/** An icon for an entry, from its title (same rules as the native palette). */
function iconFor(title: string): LucideIcon {
  const starts = (p: string) => title.startsWith(p);
  if (starts("Approve")) return Check;
  if (starts("Go to") || starts("Focus")) return ArrowRight;
  if (starts("New")) return Plus;
  if (starts("Split")) return PanelRight;
  if (starts("Close")) return X;
  if (starts("Theme")) return PaletteIcon;
  if (starts("Layout")) return LayoutGrid;
  if (starts("Settings") || starts("Open settings")) return Settings;
  if (starts("Browser") || starts("Open URL")) return Globe;
  if (starts("Workspace") || starts("Switch")) return FolderOpen;
  if (/model/i.test(title)) return Cpu;
  return Zap;
}

export function CommandPalette() {
  const { paletteQuery, closePalette } = useWorkspacesPick("paletteQuery", "closePalette");
  if (paletteQuery === null) return null;
  return <Palette initialQuery={paletteQuery} onClose={closePalette} />;
}

function Palette({ initialQuery, onClose }: { initialQuery: string; onClose: () => void }) {
  const [query, setQuery] = useState(initialQuery);
  const [selected, setSelected] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const entries = useMemo(paletteEntries, []);

  const results = useMemo(
    () =>
      entries
        .map((e) => ({ e, s: score(query, e.title) }))
        .filter((r): r is { e: (typeof entries)[number]; s: number } => r.s !== null)
        .sort((a, b) => a.s - b.s)
        .map((r) => r.e),
    [entries, query],
  );

  useEffect(() => input.current?.focus(), []);
  useEffect(() => setSelected(0), [query]);
  useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-index="${selected}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const close = () => {
    onClose();
    const focused = useWorkspaces.getState().focusedPaneId;
    if (focused) focusTerminal(focused);
  };

  const run = (index: number) => {
    const entry = results[index];
    if (!entry) return;
    onClose();
    entry.run();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") setSelected((i) => Math.min(i + 1, results.length - 1));
    else if (e.key === "ArrowUp") setSelected((i) => Math.max(i - 1, 0));
    else if (e.key === "Enter") run(selected);
    else if (e.key === "Escape") close();
    else return;
    e.preventDefault();
  };

  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex justify-center bg-black/40 pt-[12vh]" onMouseDown={close}>
      <div
        className="anim-dialog flex h-fit max-h-[min(540px,70vh)] w-[620px] flex-col isolate overflow-hidden rounded-[14px] border border-border-strong bg-panel-raised shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <label className="flex h-[50px] shrink-0 items-center gap-2.5 border-b border-border px-4 text-fg-subtle">
          <Search size={16} />
          <input
            ref={input}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Type a command…"
            className="min-w-0 flex-1 bg-transparent text-[15px] text-fg outline-none placeholder:text-fg-subtle"
          />
        </label>
        <div ref={list} className="min-h-0 flex-1 overflow-y-auto p-1.5">
          {results.length === 0 && <div className="px-3 py-6 text-center text-xs text-fg-subtle">No matching commands</div>}
          {results.map((entry, i) => {
            const Icon = iconFor(entry.title);
            const waiting = entry.id.startsWith("approve:") || entry.id.startsWith("goto:");
            return (
              <button
                key={entry.id}
                data-index={i}
                onMouseMove={() => setSelected(i)}
                onClick={() => run(i)}
                className={cn("flex h-[34px] w-full items-center gap-2.5 rounded-lg px-2.5 text-left text-[13px]", i === selected ? "bg-active text-fg" : "text-fg-muted")}
              >
                <span className={cn("grid size-6 shrink-0 place-items-center rounded-md", waiting ? "bg-warning/15 text-warning" : cn("bg-hover", i === selected ? "text-accent" : "text-fg-subtle"))}>
                  <Icon size={13} />
                </span>
                <span className="flex-1 truncate">{entry.title}</span>
                {entry.keys && <kbd className="rounded border border-border bg-panel px-1.5 py-px font-mono text-[10.5px] text-fg-subtle">{entry.keys}</kbd>}
              </button>
            );
          })}
        </div>
        <div className="flex h-[34px] shrink-0 items-center gap-3 border-t border-border px-4 text-[11px] text-fg-subtle">
          <span>↑↓ choose</span>
          <span>↵ run</span>
          <span>esc close</span>
          <span className="flex-1" />
          <span>{results.length} commands</span>
        </div>
      </div>
    </div>
  );
}
