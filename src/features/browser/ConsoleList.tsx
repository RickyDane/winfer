import { Send, Trash2 } from "lucide-react";
import { cn } from "@/lib/cn";
import { clearConsole, sendConsoleToAgent, useBrowser } from "./browser";

const LEVEL: Record<string, string> = { error: "text-danger", warning: "text-warning", info: "text-accent", log: "text-fg-muted" };

/** Console and network errors captured for the active tab. */
export function ConsoleList({ tabId }: { tabId: string }) {
  const entries = useBrowser((s) => s.console[tabId] ?? []);
  return (
    <div className="flex max-h-[45%] min-h-24 shrink-0 flex-col border-b border-border bg-panel">
      <div className="flex items-center gap-2 px-2.5 py-1.5 text-[11px] text-fg-subtle">
        <span className="flex-1">Console · {entries.length} entries</span>
        <button onClick={sendConsoleToAgent} className="flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-hover hover:text-fg" title="Paste errors into the agent's prompt">
          <Send size={11} /> Send errors to agent
        </button>
        <button onClick={() => void clearConsole()} className="rounded p-1 hover:bg-hover hover:text-fg" title="Clear">
          <Trash2 size={11} />
        </button>
      </div>
      <div className="flex-1 overflow-y-auto px-2.5 pb-2 font-mono text-[11px] leading-relaxed select-text">
        {entries.length === 0 && <div className="text-fg-subtle">Nothing logged yet.</div>}
        {entries.map((e, i) => (
          <div key={i} className="flex gap-2 border-b border-border/50 py-0.5">
            <span className={cn("w-12 shrink-0", LEVEL[e.level])}>{e.level}</span>
            <span className="min-w-0 flex-1 break-words text-fg">{e.text}</span>
            {e.source && <span className="max-w-32 shrink-0 truncate text-fg-subtle" title={e.source}>{e.source.split("/").at(-1)}</span>}
          </div>
        ))}
      </div>
    </div>
  );
}
