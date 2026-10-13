import { ArrowLeft, ArrowRight, Globe, LayoutDashboard, Monitor, Plus, RotateCw, X } from "lucide-react";
import { useState } from "react";
import { cn } from "@/lib/cn";

interface Tab {
  id: string;
  title: string;
  url: string;
  owner: "user" | "agent";
}

// Mock tabs for the M0 shell. M4 backs each tab with a Chromium CDP target.
const initialTabs: Tab[] = [
  { id: "t1", title: "Preview", url: "localhost:3000", owner: "user" },
  { id: "t2", title: "Dashboard", url: "localhost:3000/dashboard", owner: "agent" },
];

export function BrowserPanel() {
  const [tabs] = useState(initialTabs);
  const [activeId, setActiveId] = useState(tabs[0].id);
  const active = tabs.find((t) => t.id === activeId)!;

  return (
    <aside className="flex h-full w-[400px] shrink-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border border-border bg-panel">
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border px-1.5">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => setActiveId(tab.id)}
            className={cn(
              "group flex h-7 max-w-40 items-center gap-1.5 rounded-md px-2 text-xs",
              tab.id === activeId ? "bg-active text-fg" : "text-fg-muted hover:bg-hover",
            )}
          >
            {tab.owner === "agent" ? <LayoutDashboard size={12} /> : <Globe size={12} />}
            <span className="truncate">{tab.title}</span>
            {tab.owner === "agent" && (
              <span className="rounded bg-accent-soft px-1 text-[10px] text-accent" title="Opened by an agent">
                agent
              </span>
            )}
            <X size={11} className="opacity-0 group-hover:opacity-60" />
          </button>
        ))}
        <button className="ml-auto rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg" title="New tab">
          <Plus size={14} />
        </button>
      </div>

      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-border px-2 text-fg-subtle">
        <button className="rounded p-1 hover:bg-hover hover:text-fg" title="Back">
          <ArrowLeft size={14} />
        </button>
        <button className="rounded p-1 hover:bg-hover hover:text-fg" title="Forward">
          <ArrowRight size={14} />
        </button>
        <div className="mx-1 flex h-7 flex-1 items-center gap-2 rounded-md bg-panel-raised px-2.5 font-mono text-xs text-fg-muted">
          <Globe size={12} />
          <span className="flex-1 truncate">{active.url}</span>
          <RotateCw size={12} className="cursor-pointer hover:text-fg" />
        </div>
        <button className="rounded p-1 hover:bg-hover hover:text-fg" title="Device">
          <Monitor size={14} />
        </button>
      </div>

      <div className="flex flex-1 items-center justify-center text-xs text-fg-subtle">
        Browser view attaches here in M4.
      </div>
    </aside>
  );
}
