import type { CSSProperties } from "react";
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

const PATHS = {
  back: "M15 18l-6-6 6-6",
  gear: "M12 15a3 3 0 100-6 3 3 0 000 6zm7.4-3a7.4 7.4 0 00-.1-1.2l2-1.6-2-3.4-2.4 1a7.6 7.6 0 00-2-1.2L14.5 3h-4l-.4 2.6a7.6 7.6 0 00-2 1.2l-2.4-1-2 3.4 2 1.6a7.4 7.4 0 000 2.4l-2 1.6 2 3.4 2.4-1a7.6 7.6 0 002 1.2l.4 2.6h4l.4-2.6a7.6 7.6 0 002-1.2l2.4 1 2-3.4-2-1.6c.1-.4.1-.8.1-1.2z",
  send: "M12 19V5M5 12l7-7 7 7",
  stop: "M8 8h8v8H8z",
  refresh: "M20 11a8 8 0 10-2.3 5.7M20 4v7h-7",
  plus: "M12 5v14M5 12h14",
  check: "M5 12.5l4.5 4.5L19 7.5",
  x: "M6 6l12 12M18 6L6 18",
  chevron: "M6 9l6 6 6-6",
  right: "M9 6l6 6-6 6",
  down: "M12 5v14M6 13l6 6 6-6",
  terminal: "M4 6l6 6-6 6M12 18h8",
  file: "M14 3H7a2 2 0 00-2 2v14a2 2 0 002 2h10a2 2 0 002-2V8zM14 3v5h5",
  pencil: "M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4",
  search: "M11 18a7 7 0 100-14 7 7 0 000 14zM20 20l-4-4",
  globe: "M12 21a9 9 0 100-18 9 9 0 000 18zM3 12h18M12 3c2.5 2.7 3.8 5.7 3.8 9s-1.3 6.3-3.8 9c-2.5-2.7-3.8-5.7-3.8-9S9.5 5.7 12 3z",
  sparkle: "M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8zM19 17l.7 1.8 1.8.7-1.8.7L19 22l-.7-1.8-1.8-.7 1.8-.7z",
  bolt: "M13 3L5 14h6l-1 7 8-11h-6z",
  copy: "M9 9h10v10H9zM5 15V5h10",
  bell: "M6 16V11a6 6 0 1112 0v5l2 2H4zM10 21h4",
  shield: "M12 3l8 3v6c0 4.5-3.4 8.3-8 9-4.6-.7-8-4.5-8-9V6z M9 12l2 2 4-4",
  laptop: "M5 6h14v9H5zM3 18h18",
  link: "M10 14a4 4 0 005.7 0l3-3a4 4 0 00-5.7-5.7l-1 1M14 10a4 4 0 00-5.7 0l-3 3a4 4 0 005.7 5.7l1-1",
  question: "M12 21a9 9 0 100-18 9 9 0 000 18zM9.2 9a3 3 0 015.6 1c0 2-3 2.5-3 4.5M12 18h.01",
  folder: "M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z",
  minus: "M6 12h12",
  hand: "M8 13V5.5a1.5 1.5 0 013 0V12M11 11V4.5a1.5 1.5 0 013 0V12M14 11.5V6a1.5 1.5 0 013 0v8a7 7 0 01-7 7h-.5A6.5 6.5 0 013 14.5V12a1.5 1.5 0 013 0v2",
} as const;
export type IconName = keyof typeof PATHS;

export function Icon({ name, size = 20, className }: { name: IconName; size?: number; className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" width={size} height={size} fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d={PATHS[name]} />
    </svg>
  );
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

export function toolIcon(tool: string | undefined): IconName {
  const t = (tool ?? "").toLowerCase();
  if (/bash|shell|exec|command|terminal/.test(t)) return "terminal";
  if (/edit|write|patch|replace|create/.test(t)) return "pencil";
  if (/read|view|cat|open/.test(t)) return "file";
  if (/grep|glob|search|find|ls|list/.test(t)) return "search";
  if (/web|fetch|browser|url|http/.test(t)) return "globe";
  if (/task|agent|spawn/.test(t)) return "sparkle";
  return "bolt";
}

/** Drop a repeated tool name ("Bash pnpm test" → "pnpm test"). */
export const toolText = (tool: string | undefined, text: string) =>
  tool && text.toLowerCase().startsWith(`${tool.toLowerCase()} `) ? text.slice(tool.length + 1) : text;
