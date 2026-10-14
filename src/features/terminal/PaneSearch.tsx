import { ChevronDown, ChevronUp, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useWorkspaces } from "@/stores/workspaces";
import { searchTerminal } from "./xtermRegistry";

/** ⌘F: search the pane's scrollback. */
export function PaneSearch({ paneId }: { paneId: string }) {
  const setSearchPane = useWorkspaces((s) => s.setSearchPane);
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<{ index: number; count: number } | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => input.current?.focus(), []);
  useEffect(
    () => () => {
      searchTerminal(paneId, "", "clear");
    },
    [paneId],
  );

  const find = (direction: "next" | "previous", q = query) => setResult(searchTerminal(paneId, q, direction));

  return (
    <div className="anim-menu absolute top-9 right-3 z-20 flex items-center gap-1 rounded-md border border-border-strong bg-panel-raised px-2 py-1 shadow-lg" onMouseDown={(e) => e.stopPropagation()}>
      <input
        ref={input}
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          find("next", e.target.value);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") find(e.shiftKey ? "previous" : "next");
          else if (e.key === "Escape") setSearchPane(null);
          else return;
          e.preventDefault();
        }}
        placeholder="Search scrollback"
        className="w-44 bg-transparent font-mono text-xs text-fg outline-none placeholder:text-fg-subtle"
      />
      <span className="min-w-10 text-right font-mono text-[11px] text-fg-subtle">
        {query && result ? (result.count ? `${result.index + 1}/${result.count}` : "0/0") : ""}
      </span>
      <button onClick={() => find("previous")} className="rounded p-0.5 text-fg-subtle hover:text-fg" title="Previous (⇧↩)">
        <ChevronUp size={13} />
      </button>
      <button onClick={() => find("next")} className="rounded p-0.5 text-fg-subtle hover:text-fg" title="Next (↩)">
        <ChevronDown size={13} />
      </button>
      <button onClick={() => setSearchPane(null)} className="rounded p-0.5 text-fg-subtle hover:text-fg" title="Close (Esc)">
        <X size={13} />
      </button>
    </div>
  );
}
