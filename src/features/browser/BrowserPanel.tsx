import { ArrowLeft, ArrowRight, Bot, Download, Globe, Loader2, Monitor, Plus, RotateCw, Smartphone, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { useWorkspaces } from "@/stores/workspaces";
import { BrowserView } from "./BrowserView";
import {
  closeTab,
  ensureBrowser,
  history,
  installBrowser,
  newTab,
  openUrl,
  selectTab,
  setDevice,
  useBrowser,
  type Tab,
} from "./browser";

const label = (tab: Tab) => {
  if (tab.title && tab.title !== tab.url) return tab.title;
  if (!tab.url || tab.url === "about:blank") return "New tab";
  try {
    return new URL(tab.url).host || tab.url;
  } catch {
    return tab.url;
  }
};

const IconButton = ({ title, onClick, children }: { title: string; onClick: () => void; children: React.ReactNode }) => (
  <button title={title} onClick={onClick} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
    {children}
  </button>
);

export function BrowserPanel() {
  const workspaceId = useWorkspaces((s) => s.activeWorkspaceId);
  const { availability, error, tabs: allTabs, activeTab, device: devices, focusUrlBar } = useBrowser();
  const tabs = allTabs[workspaceId] ?? [];
  const active = tabs.find((t) => t.id === activeTab[workspaceId]) ?? null;
  const device = devices[workspaceId] ?? "desktop";

  const [address, setAddress] = useState("");
  const [editing, setEditing] = useState(false);
  const urlInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    void ensureBrowser(workspaceId);
  }, [workspaceId, availability]);

  useEffect(() => {
    if (!editing) setAddress(active?.url === "about:blank" ? "" : (active?.url ?? ""));
  }, [active?.url, editing]);

  useEffect(() => {
    if (focusUrlBar) {
      urlInput.current?.focus();
      urlInput.current?.select();
    }
  }, [focusUrlBar]);

  return (
    <aside className="flex h-full w-[420px] shrink-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border border-border bg-panel">
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border px-1.5">
        <div className="flex min-w-0 flex-1 gap-1 overflow-x-auto">
          {tabs.map((tab) => (
            <button
              key={tab.id}
              onClick={() => selectTab(tab.id)}
              title={tab.url}
              className={cn(
                "group flex h-7 max-w-44 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs",
                tab.id === active?.id ? "bg-active text-fg" : "text-fg-muted hover:bg-hover",
              )}
            >
              {tab.byAgent ? <Bot size={12} className="text-accent" /> : <Globe size={12} />}
              <span className="truncate">{label(tab)}</span>
              {tab.byAgent && <span className="rounded bg-accent-soft px-1 text-[10px] text-accent">agent</span>}
              <X
                size={11}
                className="shrink-0 opacity-0 group-hover:opacity-60 hover:opacity-100"
                onClick={(e) => {
                  e.stopPropagation();
                  void closeTab(tab.id);
                }}
              />
            </button>
          ))}
        </div>
        <IconButton title="New tab" onClick={() => void newTab()}>
          <Plus size={14} />
        </IconButton>
      </div>

      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-border px-2">
        <IconButton title="Back" onClick={() => void history(-1)}>
          <ArrowLeft size={14} />
        </IconButton>
        <IconButton title="Forward" onClick={() => void history(1)}>
          <ArrowRight size={14} />
        </IconButton>
        <form
          className="mx-1 flex h-7 flex-1 items-center gap-2 rounded-md bg-panel-raised px-2.5 text-fg-subtle"
          onSubmit={(e) => {
            e.preventDefault();
            if (address.trim()) void openUrl(address, { newTab: !active });
            urlInput.current?.blur();
          }}
        >
          <Globe size={12} />
          <input
            ref={urlInput}
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            onFocus={() => setEditing(true)}
            onBlur={() => setEditing(false)}
            onKeyDown={(e) => e.key === "Escape" && urlInput.current?.blur()}
            placeholder="Search or enter address (⌘L)"
            spellCheck={false}
            className="min-w-0 flex-1 bg-transparent font-mono text-xs text-fg-muted outline-none placeholder:text-fg-subtle"
          />
          <RotateCw size={12} className="cursor-pointer hover:text-fg" onClick={() => void history(0)} />
        </form>
        <IconButton title={device === "mobile" ? "Desktop view" : "Mobile view"} onClick={() => setDevice("toggle")}>
          {device === "mobile" ? <Smartphone size={14} /> : <Monitor size={14} />}
        </IconButton>
      </div>

      {availability === "ready" && active ? (
        <BrowserView key={`${workspaceId}:${active.id}`} workspaceId={workspaceId} tabId={active.id} device={device} />
      ) : (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 text-center text-xs text-fg-subtle">
          {availability === "missing" && (
            <>
              <span>No Chrome, Edge or Chromium found. winfer can download a headless browser (~100 MB) for previews and agents.</span>
              <button
                onClick={() => void installBrowser()}
                className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white hover:opacity-90"
              >
                <Download size={13} /> Download browser
              </button>
            </>
          )}
          {availability === "installing" && (
            <span className="flex items-center gap-2">
              <Loader2 size={14} className="animate-spin" /> Downloading browser…
            </span>
          )}
          {availability === "error" && <span className="text-danger">{error}</span>}
          {(availability === "unknown" || (availability === "ready" && !active)) && <span>Starting browser…</span>}
        </div>
      )}
    </aside>
  );
}
