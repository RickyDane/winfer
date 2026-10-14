import { SideTerminal } from "@/features/terminal/SideTerminal";
import { DevicePanel } from "@/features/android/DevicePanel";
import { keysFor } from "@/commands/keymap";
import { ArrowLeft, ArrowRight, Bot, Crosshair, Download, Globe, Loader2, Monitor, Plus, RotateCw, Smartphone, SquareTerminal, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { DEFAULT_BROWSER_WIDTH, useWorkspaces } from "@/stores/workspaces";
import { BrowserView } from "./BrowserView";
import { ConsoleList } from "./ConsoleList";
import {
  closeTab,
  ensureBrowser,
  history,
  installBrowser,
  newTab,
  openUrl,
  selectTab,
  setDevice,
  togglePick,
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

const MIN_WIDTH = 280;
/** Room the terminals keep when the browser is dragged wide. */
const MIN_TERMINALS = 360;

/** The gap left of the browser panel: drag to resize, double-click for the default width. */
export function BrowserResizer() {
  const setWidth = useWorkspaces((s) => s.setBrowserWidth);
  const start = (e: React.PointerEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const initial = useWorkspaces.getState().browserWidth;
    const max = window.innerWidth - MIN_TERMINALS;
    const move = (ev: PointerEvent) => setWidth(Math.min(Math.max(initial + startX - ev.clientX, MIN_WIDTH), Math.max(max, MIN_WIDTH)));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.style.cursor = "";
      document.body.classList.remove("resizing");
    };
    document.body.style.cursor = "col-resize";
    // Keeps the page canvas and terminals from eating pointer events mid-drag.
    document.body.classList.add("resizing");
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div
      onPointerDown={start}
      onDoubleClick={() => setWidth(DEFAULT_BROWSER_WIDTH)}
      title="Drag to resize · double-click to reset"
      className="group relative -mx-2 w-2 shrink-0 cursor-col-resize"
    >
      <div className="absolute inset-y-2 left-[3px] w-0.5 rounded-full bg-accent opacity-0 transition-opacity group-hover:opacity-60" />
    </div>
  );
}

export function BrowserPanel() {
  const workspaceId = useWorkspaces((s) => s.activeWorkspaceId);
  const { availability, error, tabs: allTabs, activeTab, device: devices, focusUrlBar, console: logs, consoleOpen, picking, notice } = useBrowser();
  const tabs = allTabs[workspaceId] ?? [];
  const active = tabs.find((t) => t.id === activeTab[workspaceId]) ?? null;
  const device = devices[workspaceId] ?? "desktop";
  const errors = active ? (logs[active.id] ?? []).filter((e) => e.level === "error").length : 0;

  useEffect(() => {
    if (!picking) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && void togglePick();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [picking]);

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
    <div className="flex min-h-0 flex-1 flex-col">
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
            placeholder={`Search or enter address (${keysFor({ type: "browser.focusUrl" })})`}
            spellCheck={false}
            className="min-w-0 flex-1 bg-transparent font-mono text-xs text-fg-muted outline-none placeholder:text-fg-subtle"
          />
          <RotateCw size={12} className="cursor-pointer hover:text-fg" onClick={() => void history(0)} />
        </form>
        <button
          title={`Pick an element and send it to the agent (${keysFor({ type: "browser.pick" })})`}
          onClick={() => void togglePick()}
          className={cn("rounded p-1 hover:bg-hover hover:text-fg", picking ? "bg-accent-soft text-accent" : "text-fg-subtle")}
        >
          <Crosshair size={14} />
        </button>
        <button
          title="Console and network errors"
          onClick={() => useBrowser.setState({ consoleOpen: !consoleOpen })}
          className={cn("relative rounded p-1 hover:bg-hover hover:text-fg", consoleOpen ? "text-fg" : "text-fg-subtle")}
        >
          <SquareTerminal size={14} />
          {errors > 0 && (
            <span className="absolute -top-0.5 -right-0.5 min-w-3.5 rounded-full bg-danger px-0.5 text-center text-[9px] leading-3.5 font-semibold text-white">
              {errors > 99 ? "99" : errors}
            </span>
          )}
        </button>
        <IconButton title={device === "mobile" ? "Desktop view" : "Mobile view"} onClick={() => setDevice("toggle")}>
          {device === "mobile" ? <Smartphone size={14} /> : <Monitor size={14} />}
        </IconButton>
      </div>
      {picking && <div className="shrink-0 bg-accent-soft px-3 py-1 text-[11px] text-accent">Click an element to send it to the agent · Esc to cancel</div>}
      {notice && <div className="shrink-0 bg-success/10 px-3 py-1 text-[11px] text-success">{notice}</div>}
      {consoleOpen && active && <ConsoleList tabId={active.id} />}

      {availability === "ready" && active ? (
        <BrowserView key={`${workspaceId}:${active.id}`} workspaceId={workspaceId} tabId={active.id} device={device} />
      ) : (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 text-center text-xs text-fg-subtle">
          {availability === "missing" && (
            <>
              <span>No Chrome, Edge or Chromium found. winfer can download a headless browser (~100 MB) for previews and agents.</span>
              <button
                onClick={() => void installBrowser()}
                className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-on-accent hover:opacity-90"
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
          {(availability === "unknown" || (availability === "ready" && !active)) && (
            <span className="flex items-center gap-2">
              <Loader2 size={14} className="animate-spin" /> Starting browser…
            </span>
          )}
        </div>
      )}
    </div>
  );
}

/** The right-hand panel: the browser, or a mirrored Android device. */
export function SidePanel() {
  const width = useWorkspaces((s) => s.browserWidth);
  const panel = useWorkspaces((s) => s.sidePanel);
  const setPanel = useWorkspaces((s) => s.setSidePanel);
  const tab = (active: boolean) =>
    cn("flex items-center gap-1.5 rounded-md px-2 py-0.5 text-xs transition-colors duration-150", active ? "bg-active font-medium text-fg" : "text-fg-muted hover:bg-hover hover:text-fg");
  return (
    <aside style={{ width }} className="anim-fade flex h-full shrink-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border border-border bg-panel">
      <div role="tablist" className="flex shrink-0 items-center gap-0.5 border-b border-border px-1.5 py-1">
        <button role="tab" aria-selected={panel === "browser"} onClick={() => setPanel("browser")} className={tab(panel === "browser")}>
          <Globe size={12} /> Browser
        </button>
        <button role="tab" aria-selected={panel === "device"} onClick={() => setPanel("device")} className={tab(panel === "device")}>
          <Smartphone size={12} /> Android
        </button>
        <button role="tab" aria-selected={panel === "terminal"} onClick={() => setPanel("terminal")} className={tab(panel === "terminal")}>
          <SquareTerminal size={12} /> Terminal
        </button>
      </div>
      {panel === "device" ? <DevicePanel /> : panel === "terminal" ? <SideTerminal /> : <BrowserPanel />}
    </aside>
  );
}
