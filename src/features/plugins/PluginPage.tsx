/** Plugin pages: title bar tabs and the main-area view that hosts a page's DOM. */
import {
  Atom,
  Bot,
  Camera,
  CircleDot,
  Database,
  DatabaseSearch,
  FileText,
  Gauge,
  Globe,
  KeyRound,
  Lock,
  MessageSquareQuote,
  Network,
  Puzzle,
  Send,
  Shield,
  Square,
  SquareKanban,
  Star,
  Terminal,
  Video,
  Waypoints,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { usePlugins, pluginPages, type PluginPage } from "./plugins";

/** Icons a page or panel action can name in `icon`. */
export const PAGE_ICONS: Record<string, LucideIcon> = {
  atom: Atom,
  bot: Bot,
  camera: Camera,
  record: CircleDot,
  database: Database,
  "database-search": DatabaseSearch,
  file: FileText,
  gauge: Gauge,
  globe: Globe,
  key: KeyRound,
  "key-round": KeyRound,
  lock: Lock,
  "message-square-quote": MessageSquareQuote,
  network: Network,
  puzzle: Puzzle,
  send: Send,
  shield: Shield,
  "square-kanban": SquareKanban,
  stop: Square,
  star: Star,
  terminal: Terminal,
  video: Video,
  waypoints: Waypoints,
};

export const pageIcon = (page: PluginPage): LucideIcon => PAGE_ICONS[page.icon ?? ""] ?? Puzzle;

/** Enabled plugins' pages; re-renders when plugins register or remove one. */
export function usePluginPages(): PluginPage[] {
  usePlugins((s) => s.version);
  return pluginPages();
}

/** Mounts a plugin page into the main area and cleans it up when it goes away. */
export function PluginPageView({ page }: { page: PluginPage }) {
  const ref = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setError(null);
    let cleanup: void | (() => void);
    try {
      cleanup = page.mount(el);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
    return () => {
      try {
        cleanup?.();
      } catch {
        // The page is going away either way.
      }
      el.replaceChildren();
    };
  }, [page]);

  return (
    <div className="wf-plugin-page relative min-h-0 min-w-0 flex-1 overflow-y-auto rounded-[var(--radius-pane)] border border-border bg-panel">
      {error && <p className="m-6 text-xs text-danger">This page failed to load: {error}</p>}
      <div ref={ref} className="h-full select-text" />
    </div>
  );
}
