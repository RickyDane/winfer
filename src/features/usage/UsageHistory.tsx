import { Loader2, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { cn } from "@/lib/cn";
import { relativeTime } from "@/stores/activity";
import { useWorkspaces } from "@/stores/workspaces";
import { dirLabel, fetchUsageByDir, type DirUsage } from "./taskCost";
import { costBreakdown } from "./TaskCostChip";
import { bucketTokens, fetchUsageHistory, formatCost, formatTokens, modelTotals, RANGES, SERIES, toBars, type Bar, type Range, type UsageBucket } from "./history";

const NAMES: Record<string, string> = { claude: "Claude Code", codex: "Codex", opencode: "OpenCode" };
const COLORS: Record<string, string> = {
  claude: "var(--color-series-claude)",
  codex: "var(--color-series-codex)",
  opencode: "var(--color-series-opencode)",
};

type Metric = "cost" | "tokens";

const RANGE_KEY = "winfer.usageRange";

function savedRange(): Range {
  try {
    const r = localStorage.getItem(RANGE_KEY);
    return RANGES.some((x) => x.id === r) ? (r as Range) : "7d";
  } catch {
    return "7d";
  }
}

/** A round axis maximum at or above `n`. */
function niceMax(n: number): number {
  if (n <= 0) return 1;
  const pow = 10 ** Math.floor(Math.log10(n));
  const step = [1, 2, 2.5, 5, 10].find((s) => s * pow >= n)!;
  return step * pow;
}

const format = (metric: Metric, n: number) => (metric === "cost" ? formatCost(n) : formatTokens(n));

function barLabel(bar: Bar, range: Range): string {
  const start = new Date(bar.start);
  if (range === "24h" || range === "7d") {
    const day = start.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
    const time = (t: number) => new Date(t).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    return `${day}, ${time(bar.start)}–${time(bar.end)}`;
  }
  return start.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
}

function axisLabel(t: number, range: Range): string {
  const d = new Date(t);
  return range === "24h" ? d.toLocaleTimeString(undefined, { hour: "numeric" }) : d.toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

function Chart({ bars, metric, range }: { bars: Bar[]; metric: Metric; range: Range }) {
  const [hover, setHover] = useState<number | null>(null);
  const totals = bars.map((b) => SERIES.reduce((s, p) => s + (b[metric][p] ?? 0), 0));
  const max = niceMax(Math.max(...totals));
  const ticks = [max, max / 2, 0];
  const dense = bars.length > 40;
  // Axis dates under the first, middle and last bars.
  const labelled = new Set([0, Math.floor((bars.length - 1) / 2), bars.length - 1]);
  const hovered = hover !== null ? bars[hover] : null;

  return (
    <div className="flex gap-2">
      <div className="flex h-40 flex-col justify-between py-0 text-right font-mono text-[10px] text-fg-subtle">
        {ticks.map((t) => (
          <span key={t} className="-translate-y-1/2 leading-none first:translate-y-0 last:translate-y-0">
            {format(metric, t).replace(/\.00$/, "")}
          </span>
        ))}
      </div>
      <div className="relative min-w-0 flex-1">
        <div className="relative h-40" onMouseLeave={() => setHover(null)}>
          {ticks.map((t, i) => (
            <div key={t} className={cn("absolute inset-x-0 border-t", i === ticks.length - 1 ? "border-border-strong" : "border-dashed border-border")} style={{ top: `${(i / (ticks.length - 1)) * 100}%` }} />
          ))}
          <div className={cn("absolute inset-0 flex items-end", dense ? "gap-px" : "gap-[2px]")}>
            {bars.map((bar, i) => (
              // The hit target is the full column height, not just the bar.
              <div key={bar.start} className="flex h-full min-w-0 flex-1 items-end" onMouseEnter={() => setHover(i)}>
                <div
                  className={cn("flex w-full flex-col-reverse gap-px overflow-hidden rounded-t-[4px]", hover !== null && hover !== i && "opacity-60")}
                  style={{ height: `${(totals[i] / max) * 100}%` }}
                >
                  {SERIES.map((p) => {
                    const v = bar[metric][p] ?? 0;
                    return v > 0 ? <div key={p} style={{ flexGrow: v, flexBasis: 0, background: COLORS[p], minHeight: 1 }} /> : null;
                  })}
                </div>
              </div>
            ))}
          </div>
          {hovered && hover !== null && (
            <div
              className="pointer-events-none absolute top-1 z-10 w-max min-w-40 rounded-md border border-border-strong bg-panel px-2.5 py-2 text-xs shadow-lg"
              style={hover < bars.length / 2 ? { left: `calc(${((hover + 1) / bars.length) * 100}% + 6px)` } : { right: `calc(${((bars.length - hover) / bars.length) * 100}% + 6px)` }}
            >
              <div className="mb-1 text-fg-muted">{barLabel(hovered, range)}</div>
              {SERIES.filter((p) => hovered.tokens[p]).map((p) => (
                <div key={p} className="flex items-center gap-2">
                  <span className="size-2 rounded-sm" style={{ background: COLORS[p] }} />
                  <span className="flex-1 text-fg">{NAMES[p]}</span>
                  <span className="font-mono text-fg">{formatCost(hovered.cost[p] ?? 0)}</span>
                  <span className="w-12 text-right font-mono text-fg-subtle">{formatTokens(hovered.tokens[p] ?? 0)}</span>
                </div>
              ))}
              {totals[hover] === 0 ? (
                <div className="text-fg-subtle">No usage</div>
              ) : (
                <div className="mt-1 flex items-center gap-2 border-t border-border pt-1">
                  <span className="flex-1 pl-4 text-fg-muted">Total</span>
                  <span className="font-mono text-fg">{formatCost(SERIES.reduce((s, p) => s + (hovered.cost[p] ?? 0), 0))}</span>
                  <span className="w-12 text-right font-mono text-fg-subtle">{formatTokens(SERIES.reduce((s, p) => s + (hovered.tokens[p] ?? 0), 0))}</span>
                </div>
              )}
            </div>
          )}
        </div>
        <div className="relative mt-1 h-3.5 font-mono text-[10px] text-fg-subtle">
          {[...labelled].map((i) => (
            <span
              key={i}
              className={cn("absolute", i === 0 ? "left-0" : i === bars.length - 1 ? "right-0" : "-translate-x-1/2")}
              style={i !== 0 && i !== bars.length - 1 ? { left: `${((i + 0.5) / bars.length) * 100}%` } : undefined}
            >
              {axisLabel(bars[i].start, range)}
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}

/** Spend per task: each worktree (or folder) the agents worked in, most expensive first. */
function ByTask({ days }: { days: number }) {
  const [rows, setRows] = useState<DirUsage[] | null>(null);
  const [all, setAll] = useState(false);
  const panes = useWorkspaces((s) => s.workspaces.flatMap((w) => w.panes));
  useEffect(() => {
    let live = true;
    setRows(null);
    fetchUsageByDir(days).then(
      (r) => live && setRows(r),
      () => live && setRows([]),
    );
    return () => {
      live = false;
    };
  }, [days]);

  if (!rows) return null;
  if (rows.length === 0) return null;
  const shown = all ? rows : rows.slice(0, 8);
  return (
    <div className="space-y-1.5">
      <div className="text-[11px] text-fg-subtle">By task — each worktree an agent worked in, so you can see what a feature cost across agents</div>
      <table className="w-full text-xs">
        <thead>
          <tr className="text-left text-[11px] text-fg-subtle">
            <th className="pb-1.5 font-normal">Task</th>
            <th className="pb-1.5 font-normal">Agents</th>
            <th className="pb-1.5 text-right font-normal">Last active</th>
            <th className="pb-1.5 text-right font-normal">Tokens</th>
            <th className="pb-1.5 text-right font-normal">Cost</th>
          </tr>
        </thead>
        <tbody>
          {shown.map((d) => {
            const label = dirLabel(d.dir, panes);
            const providers = [...new Set(d.models.map((m) => m.provider))];
            return (
              <tr key={d.dir} className="border-t border-border" title={`${d.dir}\n\n${costBreakdown(d)}`}>
                <td className="max-w-56 py-1.5">
                  <div className="truncate text-fg">{label.title}</div>
                  {label.detail && <div className="truncate font-mono text-[10px] text-fg-subtle">{label.detail}</div>}
                </td>
                <td className="py-1.5">
                  <span className="flex items-center gap-1.5">
                    {providers.map((p) => (
                      <span key={p} className="size-2 rounded-sm" style={{ background: COLORS[p] ?? "var(--color-fg-subtle)" }} title={NAMES[p] ?? p} />
                    ))}
                  </span>
                </td>
                <td className="py-1.5 text-right text-fg-subtle">{d.lastAt ? relativeTime(d.lastAt) : ""}</td>
                <td className="py-1.5 text-right font-mono text-fg-muted">{formatTokens(d.tokens)}</td>
                <td className="py-1.5 text-right font-mono text-fg">{d.priced || d.cost > 0 ? formatCost(d.cost) : <span className="text-fg-subtle">—</span>}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {rows.length > 8 && (
        <button onClick={() => setAll(!all)} className="text-[11px] text-fg-subtle hover:text-fg">
          {all ? "Show fewer" : `Show all ${rows.length}`}
        </button>
      )}
    </div>
  );
}

const Stat = ({ label, value, detail }: { label: string; value: string; detail?: string }) => (
  <div className="min-w-0 flex-1 rounded-lg border border-border bg-panel px-3 py-2.5">
    <div className="text-[11px] text-fg-subtle">{label}</div>
    <div className="mt-0.5 font-mono text-lg text-fg">{value}</div>
    {detail && <div className="truncate text-[11px] text-fg-subtle">{detail}</div>}
  </div>
);

/** Tokens the agents used and what they'd cost on the API, over a chosen range of the last 90 days. */
export function UsageHistory() {
  const [range, setRange] = useState<Range>(savedRange);
  const [metric, setMetric] = useState<Metric>("cost");
  // Always the full 90 days: switching ranges is instant.
  const [rows, setRows] = useState<UsageBucket[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = () => {
    setLoading(true);
    setError(null);
    fetchUsageHistory(90)
      .then(setRows, (e) => setError(String(e)))
      .finally(() => setLoading(false));
  };
  useEffect(load, []);

  const pick = (r: Range) => {
    setRange(r);
    try {
      localStorage.setItem(RANGE_KEY, r);
    } catch {
      // Not remembered; fine.
    }
  };

  const bars = useMemo(() => (rows ? toBars(rows, range) : []), [rows, range]);
  const models = useMemo(() => (rows && bars.length ? modelTotals(rows, bars[0].start) : []), [rows, bars]);
  const sum = (f: (m: (typeof models)[number]) => number) => models.reduce((s, m) => s + f(m), 0);
  const cost = sum((m) => m.cost);
  const tokens = sum(bucketTokens);
  const providers = SERIES.filter((p) => models.some((m) => m.provider === p));
  const unpriced = models.filter((m) => !m.priced);

  return (
    <div className="space-y-4">
      <p className="text-xs text-fg-muted">
        Tokens your agents used, read from Claude Code, Codex and OpenCode's logs, and what they would cost at standard API prices.
        On a subscription plan you don't pay this; it shows what the plan saves. History is kept for 90 days.
      </p>

      <div className="flex items-center gap-2">
        <div className="flex rounded-md border border-border p-0.5">
          {RANGES.map((r) => (
            <button
              key={r.id}
              onClick={() => pick(r.id)}
              className={cn("rounded px-2.5 py-1 text-xs", range === r.id ? "bg-active text-fg" : "text-fg-muted hover:text-fg")}
            >
              {r.label}
            </button>
          ))}
        </div>
        <div className="flex-1" />
        <button onClick={load} disabled={loading} title="Read new usage" className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-fg disabled:opacity-50">
          {loading ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
        </button>
      </div>

      {error && <p className="text-xs text-danger">{error}</p>}
      {!rows && !error && (
        <div className="flex items-center gap-1.5 text-xs text-fg-subtle">
          <Loader2 size={12} className="animate-spin" /> Reading usage from the agents' logs…
        </div>
      )}

      {rows && (
        <>
          <div className="flex gap-2">
            <Stat label="Estimated API cost" value={formatCost(cost)} detail={unpriced.length ? `${unpriced.length} model${unpriced.length > 1 ? "s" : ""} without a known price` : undefined} />
            <Stat
              label="Tokens"
              value={formatTokens(tokens)}
              detail={`${formatTokens(sum((m) => m.input))} in · ${formatTokens(sum((m) => m.output))} out · ${formatTokens(sum((m) => m.cacheRead))} cached`}
            />
          </div>

          <div className="space-y-2">
            <div className="flex items-center gap-3">
              <div className="flex rounded-md border border-border p-0.5">
                {(["cost", "tokens"] as const).map((m) => (
                  <button
                    key={m}
                    onClick={() => setMetric(m)}
                    className={cn("rounded px-2 py-0.5 text-[11px]", metric === m ? "bg-active text-fg" : "text-fg-muted hover:text-fg")}
                  >
                    {m === "cost" ? "Cost" : "Tokens"}
                  </button>
                ))}
              </div>
              <div className="flex flex-1 justify-end gap-3">
                {providers.map((p) => (
                  <span key={p} className="flex items-center gap-1.5 text-[11px] text-fg-muted">
                    <span className="size-2 rounded-sm" style={{ background: COLORS[p] }} />
                    {NAMES[p]}
                  </span>
                ))}
              </div>
            </div>
            <Chart bars={bars} metric={metric} range={range} />
          </div>

          {models.length === 0 ? (
            <p className="text-xs text-fg-subtle">No agent usage in this range.</p>
          ) : (
            <table className="w-full text-xs">
              <thead>
                <tr className="text-left text-[11px] text-fg-subtle">
                  <th className="pb-1.5 font-normal">Model</th>
                  <th className="pb-1.5 text-right font-normal">Input</th>
                  <th className="pb-1.5 text-right font-normal">Output</th>
                  <th className="pb-1.5 text-right font-normal">Cache read</th>
                  <th className="pb-1.5 text-right font-normal">Cache write</th>
                  <th className="pb-1.5 text-right font-normal">Cost</th>
                </tr>
              </thead>
              <tbody className="font-mono">
                {models.map((m) => (
                  <tr key={`${m.provider}/${m.model}`} className="border-t border-border">
                    <td className="max-w-44 py-1.5 font-sans">
                      <span className="flex items-center gap-1.5">
                        <span className="size-2 shrink-0 rounded-sm" style={{ background: COLORS[m.provider] ?? "var(--color-fg-subtle)" }} />
                        <span className="truncate text-fg" title={`${NAMES[m.provider] ?? m.provider} · ${m.model}`}>
                          {m.model}
                        </span>
                      </span>
                    </td>
                    <td className="py-1.5 text-right text-fg-muted">{formatTokens(m.input)}</td>
                    <td className="py-1.5 text-right text-fg-muted">{formatTokens(m.output)}</td>
                    <td className="py-1.5 text-right text-fg-muted">{formatTokens(m.cacheRead)}</td>
                    <td className="py-1.5 text-right text-fg-muted">{formatTokens(m.cacheWrite)}</td>
                    <td className="py-1.5 text-right text-fg">{m.priced ? formatCost(m.cost) : <span className="text-fg-subtle" title="No known API price">—</span>}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          <ByTask days={RANGES.find((r) => r.id === range)!.days} />
        </>
      )}
    </div>
  );
}
