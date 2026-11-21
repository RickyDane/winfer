import type { Item } from "./store";

export type DisplayRow =
  | { kind: "item"; index: number; key: string }
  | {
      kind: "work";
      key: string;
      start: number;
      end: number;
      workedMs: number | null;
      active: boolean;
    }
  | { kind: "working"; key: string };

const isWorkItem = (it: Item) => it.kind === "tool" || it.kind === "reasoning";

/** "Worked for 12s" / "Worked for 3m 24s" / "Worked for 1h 02m". */
export function fmtWorkedFor(ms: number): string {
  const t = Math.max(1, Math.round(ms / 1000));
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const sec = t % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `Worked for ${h ? `${h}h ${pad(m)}m` : m ? `${m}m ${pad(sec)}s` : `${sec}s`}`;
}

/** Duration in ms for a compacted work group, or null when timestamps are unavailable (e.g. restored history). */
export function groupDurationMs(
  items: Item[],
  start: number,
  end: number,
  workedMs: number | null,
): number | null {
  if (workedMs != null && workedMs > 0) return workedMs;
  let minStart = Infinity;
  let maxEnd = 0;
  for (let i = start; i <= end; i++) {
    const it = items[i];
    if (it.kind === "tool" || it.kind === "reasoning") {
      if (it.started > 0) minStart = Math.min(minStart, it.started);
      const ended = it.ended ?? 0;
      if (ended > maxEnd) maxEnd = ended;
    }
  }
  if (minStart < Infinity && maxEnd >= minStart) {
    return Math.max(1000, maxEnd - minStart);
  }
  return null;
}

export interface GroupSummary {
  /** The most recent tool or reasoning step in the group. */
  lastStep: Extract<Item, { kind: "tool" | "reasoning" }> | null;
  toolCount: number;
  failedCount: number;
  stepCount: number;
}

function truncateText(s: string, max: number): string {
  const clean = s.replace(/\s+/g, " ").trim();
  if (clean.length <= max) return clean;
  return `${clean.slice(0, max - 1).trimEnd()}…`;
}

function fileLabel(raw: string): string {
  const first = raw.trim().split(/\s+/)[0] ?? "";
  const base = first.split(/[\\/]/).pop() || first;
  return truncateText(base, 36);
}

function hostLabel(raw: string): string {
  const stripped = raw.trim().replace(/^https?:\/\/(www\.)?/i, "");
  return truncateText(stripped.split("/")[0] || stripped, 36);
}

function capitalizeFirst(s: string): string {
  if (!s) return s;
  const first = Array.from(s)[0];
  return first ? first.toUpperCase() + s.slice(first.length) : s;
}

/** Short, subtle status description for the active compact row (no raw tool chrome). */
export function describeStep(
  step: Extract<Item, { kind: "tool" | "reasoning" }> | null,
): string {
  return capitalizeFirst(describeStepRaw(step));
}

function describeStepRaw(
  step: Extract<Item, { kind: "tool" | "reasoning" }> | null,
): string {
  if (!step) return "Working…";
  if (step.kind === "reasoning") {
    return step.streaming ? "Thinking…" : "Working…";
  }
  const input = (step.input && typeof step.input === "object" ? step.input : null) as Record<
    string,
    unknown
  > | null;
  const desc = typeof input?.description === "string" ? input.description.trim() : "";
  const title = step.title.trim();

  switch (step.name) {
    case "Read": {
      const f = fileLabel(title);
      return f ? `Reading ${f}` : "Reading file";
    }
    case "Edit": {
      const f = fileLabel(title);
      return f ? `Editing ${f}` : "Editing file";
    }
    case "Write": {
      const f = fileLabel(title);
      return f ? `Writing ${f}` : "Writing file";
    }
    case "Grep":
      return title ? `Searching ${truncateText(title, 36)}` : "Searching files";
    case "Glob":
      return title ? `Finding ${truncateText(title, 36)}` : "Finding files";
    case "Bash": {
      if (desc) return truncateText(desc, 48);
      const cmd = title.split("\n")[0]?.trim() || "";
      return cmd ? `Running ${truncateText(cmd, 40)}` : "Running command";
    }
    case "Agent":
      if (desc) return truncateText(desc, 48);
      return title ? `Running agent · ${truncateText(title, 34)}` : "Running subagent";
    case "WebSearch":
      return title ? `Searching web · ${truncateText(title, 34)}` : "Searching the web";
    case "WebFetch": {
      const h = hostLabel(title);
      return h ? `Fetching ${h}` : "Fetching page";
    }
    case "TodoWrite":
    case "UpdatePlan":
      return "Updating plan";
    default: {
      if (desc) return truncateText(desc, 48);
      if (title) return truncateText(title, 48);
      return step.name ? `Running ${truncateText(step.name, 36)}` : "Working…";
    }
  }
}

export function summarizeGroup(items: Item[], start: number, end: number): GroupSummary {
  let lastStep: Extract<Item, { kind: "tool" | "reasoning" }> | null = null;
  let toolCount = 0;
  let failedCount = 0;
  let stepCount = 0;
  for (let i = start; i <= end; i++) {
    const it = items[i];
    if (it.kind === "tool") {
      toolCount++;
      stepCount++;
      if (it.ok === false) failedCount++;
      lastStep = it;
    } else if (it.kind === "reasoning") {
      stepCount++;
      lastStep = it;
    }
  }
  return { lastStep, toolCount, failedCount, stepCount };
}

/**
 * Groups tool calls, thinking, and intermediate assistant steps within each turn into a single
 * collapsible `work` row, leaving user messages, notices, and the final assistant reply top-level.
 */
export function buildDisplayRows(
  items: Item[],
  busy: boolean,
  compactTools: boolean,
): DisplayRow[] {
  if (!compactTools) {
    const out: DisplayRow[] = items.map((it, index) => ({ kind: "item", index, key: it.key }));
    if (busy && items.at(-1)?.kind === "user") {
      out.push({ kind: "working", key: "cw-working" });
    }
    return out;
  }

  const rows: DisplayRow[] = [];
  let i = 0;

  while (i < items.length) {
    if (items[i].kind === "user") {
      rows.push({ kind: "item", index: i, key: items[i].key });
      i++;
      // If the user message is the very last item and the agent is busy, show "Working…".
      if (i === items.length && busy) {
        rows.push({ kind: "working", key: "cw-working" });
      }
      continue;
    }

    // Collect one turn (everything up to the next user item).
    const turnStart = i;
    while (i < items.length && items[i].kind !== "user") i++;
    const turnEnd = i;
    const isLastTurn = turnEnd === items.length;

    let hasWork = false;
    let turnWorkedIdx = -1;
    for (let k = turnStart; k < turnEnd; k++) {
      if (isWorkItem(items[k])) hasWork = true;
      if (items[k].kind === "worked") turnWorkedIdx = k;
    }

    if (!hasWork) {
      for (let k = turnStart; k < turnEnd; k++) {
        rows.push({ kind: "item", index: k, key: items[k].key });
      }
      if (isLastTurn && busy) {
        const last = items[turnEnd - 1];
        if (!(last.kind === "text" && last.streaming)) {
          rows.push({ kind: "working", key: "cw-working" });
        }
      }
      continue;
    }

    const turnWorkedMs =
      turnWorkedIdx >= 0 ? (items[turnWorkedIdx] as Extract<Item, { kind: "worked" }>).ms : null;

    // Split the turn on `notice` items so warnings/errors are never hidden inside a collapsed group.
    const workRowIndices: number[] = [];
    let segStart = turnStart;
    while (segStart < turnEnd) {
      if (segStart === turnWorkedIdx) {
        segStart++;
        continue;
      }
      if (items[segStart].kind === "notice") {
        rows.push({ kind: "item", index: segStart, key: items[segStart].key });
        segStart++;
        continue;
      }

      let segEnd = segStart;
      while (segEnd < turnEnd && items[segEnd].kind !== "notice" && segEnd !== turnWorkedIdx) {
        segEnd++;
      }

      let lastWork = -1;
      for (let k = segStart; k < segEnd; k++) {
        if (isWorkItem(items[k])) lastWork = k;
      }

      if (lastWork < 0) {
        for (let k = segStart; k < segEnd; k++) {
          rows.push({ kind: "item", index: k, key: items[k].key });
        }
      } else {
        // Group from the first item in this segment through its last tool/reasoning step.
        const isTrailingInTurn =
          isLastTurn && lastWork === segEnd - 1 && (segEnd === turnEnd || segEnd === turnWorkedIdx);
        const active = busy && isTrailingInTurn;
        workRowIndices.push(rows.length);
        rows.push({
          kind: "work",
          key: `wg-${items[segStart].key}`,
          start: segStart,
          end: lastWork,
          workedMs: null,
          active,
        });
        for (let k = lastWork + 1; k < segEnd; k++) {
          rows.push({ kind: "item", index: k, key: items[k].key });
        }
      }

      segStart = segEnd;
    }

    // Attach the turn's authoritative Worked duration to its single work group (suppressing a duplicate divider).
    if (workRowIndices.length === 1 && turnWorkedMs != null) {
      const row = rows[workRowIndices[0]];
      if (row.kind === "work") row.workedMs = turnWorkedMs;
    } else if (turnWorkedIdx >= 0 && workRowIndices.length === 0) {
      rows.push({ kind: "item", index: turnWorkedIdx, key: items[turnWorkedIdx].key });
    }
  }

  return rows;
}
