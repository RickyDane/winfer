/** Tool calls and shell commands the agents ran (see src-tauri/src/tool_stats.rs). */
import { invoke } from "@tauri-apps/api/core";

export interface ToolStat {
  name: string;
  calls: number;
  failed: number;
  /** Estimated: the call's input and its result. */
  tokens: number;
  /** Summed over the `timed` calls whose time is known. */
  durationMs: number;
  maxMs: number;
  timed: number;
}

export interface ToolFailure {
  tool: string;
  provider: string;
  at: number;
  input: string;
  error: string;
  cwd: string | null;
  durationMs: number | null;
}

export interface ToolReport {
  calls: number;
  failed: number;
  tokens: number;
  durationMs: number;
  sessions: number;
  tools: ToolStat[];
  /** Shell commands by program. */
  commands: ToolStat[];
  failures: ToolFailure[];
}

export const fetchToolStats = (days: number) => invoke<ToolReport>("usage_tool_stats", { days });

/** A tool or command's time: "320ms", "4.2s", "3m 10s", "1h 5m". */
export function elapsed(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s < 10 ? s.toFixed(1) : Math.round(s)}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${Math.round(s % 60)}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

export const failRate = (s: Pick<ToolStat, "calls" | "failed">) => (s.calls ? (s.failed * 100) / s.calls : 0);

export type ToolSort = "calls" | "failed" | "time" | "tokens";

export function sortStats(stats: ToolStat[], by: ToolSort): ToolStat[] {
  const key: Record<ToolSort, (s: ToolStat) => number> = {
    calls: (s) => s.calls,
    failed: (s) => s.failed,
    time: (s) => s.durationMs,
    tokens: (s) => s.tokens,
  };
  return [...stats].sort((a, b) => key[by](b) - key[by](a) || b.calls - a.calls || a.name.localeCompare(b.name));
}
