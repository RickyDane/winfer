// Compacted tool call grouping and step descriptions — port of native-gpui's
// `code_view.rs` (`build_display_rows`, `describe_step`, `group_duration_ms`, `elapsed`).
import { useSyncExternalStore } from "react";
import type { Message } from "../protocol";

const stripToolPrefix = (tool: string | undefined, text: string) =>
  tool && text.toLowerCase().startsWith(`${tool.toLowerCase()} `) ? text.slice(tool.length + 1) : text;

export type DisplayRow =
  | { kind: "msg"; msg: Message }
  | { kind: "tools"; key: string; msgs: Message[] }
  | { kind: "work-group"; key: string; msgs: Message[]; active: boolean; thinking: boolean };

/** Mirrors `code_view.rs::elapsed`. */
export function elapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}m`;
}

/** Mirrors `code_view.rs::group_duration_ms`. Uses `ts`..`ended` when present, else `ts` span. */
export function groupDurationMs(msgs: Message[]): number | null {
  let minStart = Number.POSITIVE_INFINITY;
  let maxEnd = 0;
  for (const m of msgs) {
    if (m.ts && m.ts > 0) minStart = Math.min(minStart, m.ts);
    if (m.ended && m.ended > 0) maxEnd = Math.max(maxEnd, m.ended);
  }
  if (minStart < Number.POSITIVE_INFINITY && maxEnd >= minStart) {
    return Math.max(maxEnd - minStart, 1000);
  }
  // Fallback when transcripts only carry start timestamps (e.g. Claude/Codex session files).
  let maxTs = 0;
  for (const m of msgs) {
    if (m.ts && m.ts > 0) maxTs = Math.max(maxTs, m.ts);
  }
  if (minStart < Number.POSITIVE_INFINITY && maxTs > minStart) {
    return Math.max(maxTs - minStart, 1000);
  }
  return null;
}

function truncateChars(s: string, max: number): string {
  const clean = s.trim().replace(/\s+/g, " ");
  const chars = [...clean];
  if (chars.length <= max) return clean;
  let out = chars.slice(0, Math.max(0, max - 1)).join("");
  while (out.endsWith(" ")) out = out.slice(0, -1);
  return `${out}…`;
}

function fileLabel(raw: string): string {
  const first = raw.trim().split(/\s+/)[0] ?? "";
  const parts = first.split(/[/\\]/);
  const base = parts[parts.length - 1] ?? first;
  return truncateChars(base, 36);
}

function hostLabel(raw: string): string {
  const stripped = raw
    .trim()
    .replace(/^https?:\/\//i, "")
    .replace(/^www\./i, "");
  const host = stripped.split("/")[0] ?? stripped;
  return truncateChars(host, 36);
}

function capitalizeFirst(s: string): string {
  if (!s) return s;
  const [first, ...rest] = [...s];
  return first.toUpperCase() + rest.join("");
}

/** Short, subtle status description for the active compact row (port of `describe_step`). */
export function describeStep(step: Message | null, thinking = false): string {
  return capitalizeFirst(describeStepRaw(step, thinking));
}

function describeStepRaw(step: Message | null, thinking: boolean): string {
  if (thinking) return "Thinking…";
  if (!step) return "Working…";
  if (step.role === "thinking") return "Thinking…";
  if (step.role !== "tool") return "Working…";

  const name = (step.tool ?? "").trim();
  const desc = (step.desc ?? "").trim();
  const title = stripToolPrefix(step.tool, step.text).trim();
  const lower = name.toLowerCase();

  switch (lower) {
    case "read":
    case "ls":
    case "list": {
      const f = fileLabel(title);
      return f ? `Reading ${f}` : "Reading file";
    }
    case "edit":
    case "multiedit":
    case "notebookedit": {
      const f = fileLabel(title);
      return f ? `Editing ${f}` : "Editing file";
    }
    case "write": {
      const f = fileLabel(title);
      return f ? `Writing ${f}` : "Writing file";
    }
    case "grep":
      return title ? `Searching ${truncateChars(title, 36)}` : "Searching files";
    case "glob":
    case "find":
      return title ? `Finding ${truncateChars(title, 36)}` : "Finding files";
    case "bash":
    case "shell":
    case "powershell": {
      if (desc) return truncateChars(desc, 48);
      const cmd = (title.split("\n")[0] ?? "").trim();
      return cmd ? `Running ${truncateChars(cmd, 40)}` : "Running command";
    }
    case "agent":
    case "task":
      if (desc) return truncateChars(desc, 48);
      return title ? `Running agent · ${truncateChars(title, 34)}` : "Running subagent";
    case "websearch":
    case "codesearch":
      return title ? `Searching web · ${truncateChars(title, 34)}` : "Searching the web";
    case "webfetch":
    case "fetch": {
      const h = hostLabel(title);
      return h ? `Fetching ${h}` : "Fetching page";
    }
    case "todowrite":
    case "updateplan":
    case "todos":
    case "taskcreate":
    case "taskupdate":
      return "Updating plan";
    case "askuserquestion":
    case "question":
      return "Asking a question";
    case "exitplanmode":
      return "Presenting plan";
    case "background":
      return title ? truncateChars(title, 48) : "Managing background task";
    default:
      if (lower.endsWith(" · visualize") || lower === "visualize") {
        return title ? `Visualizing ${truncateChars(title, 36)}` : "Creating visualization";
      }
      if (desc) return truncateChars(desc, 48);
      if (title) return truncateChars(title, 48);
      if (name) return `Running ${truncateChars(name, 36)}`;
      return "Working…";
  }
}

/**
 * Groups transcript messages into display rows, matching `native-gpui`'s `build_display_rows`:
 * - When `compactTools` is false: consecutive tool calls stay in a flat `tools` run.
 * - When `compactTools` is true: each run of tool calls between text messages in a turn becomes a
 *   `work-group`, except a finished 1-tool run which shows directly. A trailing run while the
 *   agent is working stays an active `work-group` (even with 1 step) and suppresses a duplicate
 *   bottom `WorkingRow`.
 */
export function buildDisplayRows(
  messages: Message[],
  busy: boolean,
  thinking: boolean,
  compactTools: boolean,
): { rows: DisplayRow[]; hasLiveGroup: boolean } {
  if (!compactTools) {
    const rows: DisplayRow[] = [];
    for (const msg of messages) {
      const last = rows[rows.length - 1];
      if (msg.role === "tool") {
        if (last?.kind === "tools") last.msgs.push(msg);
        else rows.push({ kind: "tools", key: `t-${msg.id}`, msgs: [msg] });
      } else {
        rows.push({ kind: "msg", msg });
      }
    }
    return { rows, hasLiveGroup: false };
  }

  const rows: DisplayRow[] = [];
  let i = 0;
  while (i < messages.length) {
    if (messages[i].role === "user") {
      rows.push({ kind: "msg", msg: messages[i] });
      i++;
      continue;
    }

    const turnStart = i;
    while (i < messages.length && messages[i].role !== "user") i++;
    const turnEnd = i;
    const isLastTurn = turnEnd === messages.length;

    let k = turnStart;
    while (k < turnEnd) {
      if (messages[k].role !== "tool") {
        rows.push({ kind: "msg", msg: messages[k] });
        k++;
        continue;
      }
      const start = k;
      while (k < turnEnd && messages[k].role === "tool") k++;
      const slice = messages.slice(start, k);
      const active = busy && isLastTurn && k === turnEnd;
      if (slice.length === 1 && !active) {
        rows.push({ kind: "tools", key: `t-${slice[0].id}`, msgs: slice });
      } else {
        rows.push({
          kind: "work-group",
          key: `wg-${slice[0].id}`,
          msgs: slice,
          active,
          thinking: active && thinking,
        });
      }
    }
  }

  const last = rows[rows.length - 1];
  const hasLiveGroup = last?.kind === "work-group" && last.active;
  return { rows, hasLiveGroup };
}

/** Label for a finished compacted work group: `"Worked for 12s · 4 tool calls"` or `"Worked · 4 tool calls"`. */
export function workGroupSummary(msgs: Message[]): string {
  const count = msgs.length;
  const unit = count === 1 ? "tool call" : "tool calls";
  const ms = groupDurationMs(msgs);
  return ms !== null ? `Worked for ${elapsed(Math.max(ms, 1000))} · ${count} ${unit}` : `Worked · ${count} ${unit}`;
}

/* ---------- Phone-side preference (defaults to desktop's compactTools setting) ---------- */
const PREF_KEY = "winfer.compactTools";
const listeners = new Set<() => void>();

function readOverride(): boolean | null {
  try {
    const v = localStorage.getItem(PREF_KEY);
    if (v === "true") return true;
    if (v === "false") return false;
  } catch {
    /* private mode / node test */
  }
  return null;
}

let cachedOverride: boolean | null = readOverride();

export function setCompactToolsOverride(value: boolean) {
  cachedOverride = value;
  try {
    localStorage.setItem(PREF_KEY, value ? "true" : "false");
  } catch {
    /* private mode */
  }
  listeners.forEach((fn) => fn());
}

const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => void listeners.delete(fn);
};

export function useCompactTools(desktopDefault: boolean): boolean {
  const override = useSyncExternalStore(subscribe, () => cachedOverride, () => null);
  return override ?? desktopDefault;
}
