import type { CSSProperties } from "react";
import {
  ArrowDown, ArrowUp, Bell, Bot, Brain, Bug, Check, ChevronDown, ChevronLeft, ChevronRight, CircleAlert, CircleDashed, CircleQuestionMark, Compass, Copy, Ellipsis,
  FilePen, FileText, FlaskConical, Folder, FolderSearch, Globe, GitBranch, Hand, Info, Laptop, Lightbulb, Link, Minus, Pencil, Plus, RefreshCw, Search, Settings,
  ShieldQuestionMark, Sparkles, Square, SquareTerminal, Wrench, X, Zap,
} from "lucide-react";
import type { ClientState } from "../client";
// Official marks, shared with the desktop's ProviderIcon.
import { PROVIDER_LOGOS } from "../../../src/features/providers/logos";
import { vtName } from "../hooks";
import type { AgentStatus, Pane } from "../protocol";

export const STATUS_LABEL: Record<AgentStatus, string> = {
  "needs-input": "Needs you",
  error: "Error",
  done: "Done",
  working: "Working",
  idle: "Idle",
};

// The desktop's icon set (lucide), by the names the screens use.
const ICONS = {
  back: ChevronLeft,
  gear: Settings,
  send: ArrowUp,
  stop: Square,
  refresh: RefreshCw,
  plus: Plus,
  check: Check,
  x: X,
  chevron: ChevronDown,
  right: ChevronRight,
  down: ArrowDown,
  terminal: SquareTerminal,
  file: FileText,
  pencil: Pencil,
  edit: FilePen,
  search: Search,
  folderSearch: FolderSearch,
  globe: Globe,
  sparkle: Sparkles,
  bolt: Zap,
  copy: Copy,
  bell: Bell,
  shield: ShieldQuestionMark,
  laptop: Laptop,
  link: Link,
  question: CircleQuestionMark,
  folder: Folder,
  minus: Minus,
  hand: Hand,
  compass: Compass,
  bug: Bug,
  flask: FlaskConical,
  bulb: Lightbulb,
  branch: GitBranch,
  more: Ellipsis,
  info: Info,
  alert: CircleAlert,
  bot: Bot,
  wrench: Wrench,
  brain: Brain,
  dashed: CircleDashed,
} as const;
export type IconName = keyof typeof ICONS;

export function Icon({ name, size = 20, className }: { name: IconName; size?: number; className?: string }) {
  const C = ICONS[name];
  return <C className={className} size={size} strokeWidth={2} aria-hidden />;
}

/** `mark`: the logo's color on the neutral avatar tile. */
type Brand = { name: string; mark: string };
const PROVIDERS: Record<string, Brand> = {
  claude: { name: "Claude Code", mark: "#d97757" },
  codex: { name: "Codex", mark: "#e6e6ea" },
  opencode: { name: "OpenCode", mark: "#f5b544" },
  winfer: { name: "winfer code", mark: "#6d7cff" },
  pi: { name: "pi", mark: "#7ee787" },
  agy: { name: "Antigravity", mark: "#4f8cff" },
  grok: { name: "Grok", mark: "#e6e6ea" },
  cursor: { name: "Cursor", mark: "#c9c9ff" },
};
export const providerInfo = (p: string | null): Brand | null => (p ? (PROVIDERS[p] ?? { name: p, mark: "#9a9aa3" }) : null);

export function ProviderMark({ provider, size = 20 }: { provider: string | null; size?: number }) {
  const d = provider ? PROVIDER_LOGOS[provider] : undefined;
  if (!d) return provider ? <span className="mark-letter">{provider[0].toUpperCase()}</span> : <Icon name="terminal" size={size} />;
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} fill="currentColor" fillRule="evenodd" clipRule="evenodd" aria-hidden>
      <path d={d} />
    </svg>
  );
}

/** A neutral tile with the agent's mark; a dot in the corner says what it's doing. */
export function Avatar({ pane, small, morph }: { pane: Pane; small?: boolean; morph?: boolean }) {
  const info = providerInfo(pane.provider);
  const style = { color: info?.mark, viewTransitionName: morph ? vtName("av", pane.id) : undefined } as CSSProperties;
  return (
    <span className="avatar" data-small={small || undefined} data-status={pane.status} style={style} aria-hidden>
      <ProviderMark provider={pane.provider} size={small ? 13 : 17} />
      {pane.status !== "idle" && <span className="avatar-dot" />}
    </span>
  );
}

export type Link = "online" | "away" | "connecting" | "offline";
export function linkState(state: ClientState): Link {
  if (state.socket === "open" && state.desktopOnline) return "online";
  if (state.socket === "open" && state.desktopOnline === false) return "away";
  if (state.socket === "connecting" || state.desktopOnline === null) return "connecting";
  return "offline";
}

/** Inline card for connection trouble; nothing when all is well. */
export function ConnNotice({ state, onRetry }: { state: ClientState; onRetry: () => void }) {
  const link = linkState(state);
  if (state.error)
    return (
      <div className="notice" data-tone="danger" role="status">
        <Icon name="x" />
        <p>{state.error}</p>
        {state.socket === "closed" && (
          <button className="btn sm" onClick={onRetry}>
            Retry
          </button>
        )}
      </div>
    );
  if (link === "away")
    return (
      <div className="notice" data-tone="warn" role="status">
        <Icon name="laptop" />
        <p>Your desktop is offline. Open winfer on your computer and this reconnects by itself.</p>
      </div>
    );
  if (link === "offline")
    return (
      <div className="notice" role="status">
        <Icon name="refresh" />
        <p>Offline. Reconnecting…</p>
        <button className="btn sm" onClick={onRetry}>
          Retry
        </button>
      </div>
    );
  return null;
}

/** A tool's icon and tint, as the desktop's chat shows them. */
export function toolIcon(tool: string | undefined): IconName {
  const t = tool ?? "";
  if (t === "Read") return "file";
  if (t === "Grep") return "search";
  if (t === "Glob") return "folderSearch";
  if (t === "Edit" || t === "Write") return "edit";
  if (t === "Agent" || t === "Task") return "bot";
  const l = t.toLowerCase();
  if (/bash|shell|exec|command|terminal/.test(l)) return "terminal";
  if (/edit|write|patch|replace|create/.test(l)) return "edit";
  if (/read|view|cat|open/.test(l)) return "file";
  if (/grep|glob|search|find|ls|list/.test(l)) return "search";
  if (/web|fetch|browser|url|http/.test(l)) return "globe";
  if (/task|agent|spawn/.test(l)) return "bot";
  return "wrench";
}

export function toolTint(tool: string | undefined): string {
  const t = tool ?? "";
  if (/^(Read|Grep|Glob|WebFetch|WebSearch)$/.test(t)) return "var(--accent)";
  if (/^(Edit|Write)$/.test(t)) return "var(--ok)";
  if (/bash|shell|exec|command/i.test(t)) return "var(--warn)";
  if (/^(Agent|Task)$/.test(t)) return "var(--claude)";
  return "var(--muted)";
}

/** Drop a repeated tool name ("Bash pnpm test" → "pnpm test"). */
export const toolText = (tool: string | undefined, text: string) =>
  tool && text.toLowerCase().startsWith(`${tool.toLowerCase()} `) ? text.slice(tool.length + 1) : text;
