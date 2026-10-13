import { useEffect, useMemo, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { useWorkspaces } from "@/stores/workspaces";
import { focusTerminal } from "@/features/terminal/xtermRegistry";
import { paletteEntries, score } from "./entries";

export function CommandPalette() {
  const { paletteQuery, closePalette } = useWorkspaces();
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
    <div className="fixed inset-0 z-50 flex justify-center bg-black/40 pt-[12vh]" onMouseDown={close}>
      <div
        className="flex h-fit max-h-[60vh] w-[560px] flex-col overflow-hidden rounded-xl border border-border-strong bg-panel-raised shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <input
          ref={input}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Type a command…"
          className="h-11 border-b border-border bg-transparent px-4 text-sm text-fg outline-none placeholder:text-fg-subtle"
        />
        <div ref={list} className="overflow-y-auto p-1.5">
          {results.length === 0 && <div className="px-3 py-6 text-center text-xs text-fg-subtle">No matching commands</div>}
          {results.map((entry, i) => (
            <button
              key={entry.id}
              data-index={i}
              onMouseMove={() => setSelected(i)}
              onClick={() => run(i)}
              className={cn(
                "flex w-full items-center rounded-md px-3 py-2 text-left text-[13px]",
                i === selected ? "bg-active text-fg" : "text-fg-muted",
              )}
            >
              <span className="flex-1 truncate">{entry.title}</span>
              {entry.keys && <kbd className="font-mono text-[11px] text-fg-subtle">{entry.keys}</kbd>}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
