/** Plugin page tabs: pinned pages sit in the title bar, the rest in the Apps menu. */
import { Blocks, ChevronDown, Search, Settings, Star } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { execute } from "@/commands/bus";
import { ContextMenu } from "@/components/ContextMenu";
import { useWorkspaces } from "@/stores/workspaces";
import { pageIcon, usePluginPages } from "./PluginPage";
import { pluginPages, type PluginPage } from "./plugins";

/** Pages pinned before the user has chosen any. */
const DEFAULT_PINNED = 3;

/** Stored pins, or the first few pages while the user hasn't chosen. Keys of absent pages are kept. */
function storedPins(pages: PluginPage[]): string[] {
  return useWorkspaces.getState().pinnedPages ?? pages.slice(0, DEFAULT_PINNED).map((p) => p.key);
}

/** Pinned pages that are currently registered, in pin order. */
export function pinnedPages(pages: PluginPage[], pinned: string[] | null): PluginPage[] {
  const keys = pinned ?? pages.slice(0, DEFAULT_PINNED).map((p) => p.key);
  return keys.flatMap((key) => pages.find((p) => p.key === key) ?? []);
}

export function togglePin(key: string) {
  const pins = storedPins(pluginPages());
  useWorkspaces.getState().setPinnedPages(pins.includes(key) ? pins.filter((k) => k !== key) : [...pins, key]);
}

/** Moves a pinned page one step among the pinned pages that are shown. */
function movePin(key: string, step: -1 | 1) {
  const pages = pluginPages();
  const pins = storedPins(pages);
  const shown = pins.filter((k) => pages.some((p) => p.key === k));
  const other = shown[shown.indexOf(key) + step];
  if (!other) return;
  const a = pins.indexOf(key);
  const b = pins.indexOf(other);
  const next = [...pins];
  [next[a], next[b]] = [next[b], next[a]];
  useWorkspaces.getState().setPinnedPages(next);
}

const openPage = (page: PluginPage) => execute({ type: "ui.view", view: `page:${page.key}` });
const managePlugins = () => execute({ type: "ui.settings", page: "plugins" });

export function PageTabs({ tab }: { tab: (active: boolean) => string }) {
  const pages = usePluginPages();
  const view = useWorkspaces((s) => s.view);
  const pinned = pinnedPages(pages, useWorkspaces((s) => s.pinnedPages));
  const [menu, setMenu] = useState<{ x: number; y: number; page: PluginPage } | null>(null);
  if (!pages.length) return null;
  const unpinned = pages.filter((p) => !pinned.includes(p));
  const index = (page: PluginPage) => pinned.indexOf(page);
  return (
    <>
      {pinned.map((page) => {
        const Icon = pageIcon(page);
        const selected = view === `page:${page.key}`;
        return (
          <button
            key={page.key}
            role="tab"
            aria-selected={selected}
            onClick={() => openPage(page)}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({ x: e.clientX, y: e.clientY, page });
            }}
            className={tab(selected)}
            title={page.title}
          >
            <Icon size={12} />
            {/* Narrow windows show just the icon; the title is in the tooltip. */}
            <span className="max-w-32 truncate max-xl:hidden">{page.title}</span>
          </button>
        );
      })}
      <AppsMenu pages={pages} pinned={pinned} unpinned={unpinned} tab={tab} />
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          title={menu.page.title}
          onClose={() => setMenu(null)}
          items={[
            { label: "Move left", run: () => movePin(menu.page.key, -1), disabled: index(menu.page) === 0 },
            { label: "Move right", run: () => movePin(menu.page.key, 1), disabled: index(menu.page) === pinned.length - 1 },
            { label: "Unpin from title bar", run: () => togglePin(menu.page.key) },
            "separator",
            { label: "Manage plugins…", run: managePlugins },
          ]}
        />
      )}
    </>
  );
}

/** The Apps tab: every plugin page, with a star to pin it to the title bar. */
function AppsMenu({ pages, pinned, unpinned, tab }: { pages: PluginPage[]; pinned: PluginPage[]; unpinned: PluginPage[]; tab: (active: boolean) => string }) {
  const view = useWorkspaces((s) => s.view);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    input.current?.focus();
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    const blur = () => setOpen(false);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
      window.removeEventListener("blur", blur);
    };
  }, [open]);

  // An open page that isn't pinned shows on the Apps tab, so it's clear where you are.
  const current = unpinned.find((p) => view === `page:${p.key}`);
  const CurrentIcon = current ? pageIcon(current) : Blocks;
  const q = query.trim().toLowerCase();
  const match = (p: PluginPage) => !q || p.title.toLowerCase().includes(q) || p.pluginId.toLowerCase().includes(q);
  const shownPinned = pinned.filter(match);
  const shownOther = unpinned.filter(match);
  const choose = (page: PluginPage) => {
    setOpen(false);
    setQuery("");
    openPage(page);
  };

  return (
    <div ref={root} className="relative">
      <button
        role="tab"
        aria-selected={!!current}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className={tab(!!current || open)}
        title={current ? `${current.title} · all plugin pages` : "All plugin pages"}
      >
        <CurrentIcon size={12} />
        <span className="max-w-32 truncate max-lg:hidden">{current ? current.title : "Apps"}</span>
        {!current && unpinned.length > 0 && <span className="font-mono text-[10px] text-fg-subtle">{unpinned.length}</span>}
        <ChevronDown size={11} className="text-fg-subtle" />
      </button>
      {open && (
        <div role="menu" className="anim-menu absolute left-1/2 top-full z-[70] mt-1.5 w-64 -translate-x-1/2 rounded-lg border border-border-strong bg-panel-raised p-1 shadow-xl">
          {pages.length > 6 && (
            <div className="mb-1 flex items-center gap-1.5 rounded-md border border-border px-2 py-1">
              <Search size={12} className="text-fg-subtle" />
              <input
                ref={input}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => {
                  const first = shownPinned[0] ?? shownOther[0];
                  if (e.key === "Enter" && first) choose(first);
                }}
                placeholder="Filter…"
                className="w-full bg-transparent text-xs text-fg outline-none placeholder:text-fg-subtle"
              />
            </div>
          )}
          {shownPinned.map((page) => (
            <AppRow key={page.key} page={page} pinned selected={view === `page:${page.key}`} onOpen={choose} />
          ))}
          {shownPinned.length > 0 && shownOther.length > 0 && <div className="my-1 h-px bg-border" />}
          {shownOther.map((page) => (
            <AppRow key={page.key} page={page} pinned={false} selected={view === `page:${page.key}`} onOpen={choose} />
          ))}
          {!shownPinned.length && !shownOther.length && <div className="px-2 py-1.5 text-xs text-fg-subtle">No matching pages</div>}
          <div className="my-1 h-px bg-border" />
          <button
            role="menuitem"
            onClick={() => {
              setOpen(false);
              managePlugins();
            }}
            className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs text-fg-muted hover:bg-hover hover:text-fg"
          >
            <Settings size={12} />
            Manage plugins…
          </button>
        </div>
      )}
    </div>
  );
}

function AppRow({ page, pinned, selected, onOpen }: { page: PluginPage; pinned: boolean; selected: boolean; onOpen: (page: PluginPage) => void }) {
  const Icon = pageIcon(page);
  return (
    <div className={cn("group flex items-center rounded-md", selected ? "bg-active" : "hover:bg-hover")}>
      <button role="menuitem" onClick={() => onOpen(page)} className={cn("flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left text-xs", selected ? "font-medium text-fg" : "text-fg-muted hover:text-fg")}>
        <Icon size={12} className="shrink-0" />
        <span className="truncate">{page.title}</span>
      </button>
      <button
        onClick={() => togglePin(page.key)}
        className={cn("mr-1 rounded p-1 hover:bg-hover", pinned ? "text-warning" : "text-fg-subtle opacity-0 group-hover:opacity-100 focus:opacity-100")}
        title={pinned ? "Unpin from title bar" : "Pin to title bar"}
        aria-label={pinned ? `Unpin ${page.title}` : `Pin ${page.title}`}
        aria-pressed={pinned}
      >
        <Star size={12} fill={pinned ? "currentColor" : "none"} />
      </button>
    </div>
  );
}
