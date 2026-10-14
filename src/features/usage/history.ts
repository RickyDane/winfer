/**
 * Token use over time and what it would cost at API prices (see src-tauri/src/usage_history.rs):
 * hourly rows per model from the agents' logs, kept for 90 days, grouped here for the chart.
 */
import { invoke } from "@tauri-apps/api/core";

export interface UsageBucket {
  /** Start of the hour (unix ms). */
  hour: number;
  provider: string;
  model: string;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  cost: number;
  /** False when the model has no known price (its cost counts as 0). */
  priced: boolean;
}

export type Range = "24h" | "7d" | "30d" | "90d";

export const RANGES: { id: Range; label: string; days: number; stepHours: number }[] = [
  { id: "24h", label: "24 hours", days: 1, stepHours: 1 },
  { id: "7d", label: "7 days", days: 7, stepHours: 6 },
  { id: "30d", label: "30 days", days: 30, stepHours: 24 },
  { id: "90d", label: "90 days", days: 90, stepHours: 24 },
];

/** Agents in chart order (fixed, so a filter never repaints a series). */
export const SERIES = ["claude", "codex", "opencode"] as const;

export const fetchUsageHistory = (days: number) => invoke<UsageBucket[]>("usage_history", { days });

export const bucketTokens = (b: Pick<UsageBucket, "input" | "output" | "cacheRead" | "cacheWrite">) => b.input + b.output + b.cacheRead + b.cacheWrite;

export interface Bar {
  start: number;
  end: number;
  /** Per provider. */
  cost: Record<string, number>;
  tokens: Record<string, number>;
}

/** Start of the local step (hour, 6 hours or day) `at` falls in. */
function stepStart(at: number, stepHours: number): number {
  const d = new Date(at);
  d.setMinutes(0, 0, 0);
  if (stepHours >= 24) d.setHours(0);
  else d.setHours(d.getHours() - (d.getHours() % stepHours));
  return d.getTime();
}

function addHours(at: number, hours: number): number {
  const d = new Date(at);
  // Calendar arithmetic, so days stay at midnight across daylight saving changes.
  if (hours >= 24) d.setDate(d.getDate() + hours / 24);
  else d.setHours(d.getHours() + hours);
  return d.getTime();
}

/** The range's bars, oldest first, the last one holding `now`; empty steps included. */
export function toBars(rows: UsageBucket[], range: Range, now = Date.now()): Bar[] {
  const { days, stepHours } = RANGES.find((r) => r.id === range)!;
  const count = Math.round((days * 24) / stepHours);
  const last = stepStart(now, stepHours);
  const bars: Bar[] = [];
  let start = last;
  for (let i = 0; i < count - 1; i++) start = addHours(start, -stepHours);
  for (let i = 0; i < count; i++) {
    const end = addHours(start, stepHours);
    bars.push({ start, end, cost: {}, tokens: {} });
    start = end;
  }
  const first = bars[0].start;
  for (const r of rows) {
    if (r.hour < first) continue;
    // Bars are few; a linear search from the end is plenty.
    let i = bars.length - 1;
    while (i > 0 && bars[i].start > r.hour) i--;
    const bar = bars[i];
    bar.cost[r.provider] = (bar.cost[r.provider] ?? 0) + r.cost;
    bar.tokens[r.provider] = (bar.tokens[r.provider] ?? 0) + bucketTokens(r);
  }
  return bars;
}

export interface ModelTotal {
  provider: string;
  model: string;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  cost: number;
  priced: boolean;
}

/** Totals per model since `since`, most expensive (then most tokens) first. */
export function modelTotals(rows: UsageBucket[], since: number): ModelTotal[] {
  const by = new Map<string, ModelTotal>();
  for (const r of rows) {
    if (r.hour < since) continue;
    const key = `${r.provider}\u0000${r.model}`;
    const t = by.get(key) ?? { provider: r.provider, model: r.model, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0, priced: true };
    t.input += r.input;
    t.output += r.output;
    t.cacheRead += r.cacheRead;
    t.cacheWrite += r.cacheWrite;
    t.cost += r.cost;
    t.priced &&= r.priced;
    by.set(key, t);
  }
  return [...by.values()].sort((a, b) => b.cost - a.cost || bucketTokens(b) - bucketTokens(a));
}

export function formatTokens(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(n >= 1e10 ? 1 : 2)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e7 ? 1 : 2)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(n >= 1e4 ? 0 : 1)}K`;
  return String(Math.round(n));
}

export function formatCost(n: number): string {
  if (n === 0) return "$0";
  if (n < 0.01) return "<$0.01";
  if (n >= 1000) return `$${Math.round(n).toLocaleString("en-US")}`;
  return `$${n.toFixed(2)}`;
}
