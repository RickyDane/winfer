import { RefreshCw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Spinner } from "@/components/Spinner";
import { cn } from "@/lib/cn";
import { relativeTime } from "@/stores/activity";
import { Segmented, useSaved } from "./controls";
import { formatTokens, RANGES, type Range } from "./history";
import { elapsed, failRate, fetchToolStats, sortStats, type ToolReport, type ToolSort, type ToolStat } from "./toolStats";
import { Stat } from "./UsageHistory";

const SORTS: { id: ToolSort; label: string }[] = [
  { id: "calls", label: "Calls" },
  { id: "failed", label: "Failed" },
  { id: "time", label: "Time" },
  { id: "tokens", label: "Tokens" },
];

const SHOWN = 12;

/** Calls per tool or command, with failures, time and estimated tokens. */
function StatTable({ title, stats, sort, prefix }: { title: string; stats: ToolStat[]; sort: ToolSort; prefix?: string }) {
  const [all, setAll] = useState(false);
  const rows = useMemo(() => sortStats(stats, sort), [stats, sort]);
  const shown = all ? rows : rows.slice(0, SHOWN);
  const max = Math.max(1, ...rows.map((s) => (sort === "time" ? s.durationMs : sort === "tokens" ? s.tokens : sort === "failed" ? s.failed : s.calls)));
  if (!rows.length) return null;
  return (
    <div className="space-y-1.5">
      <div className="text-[11px] font-medium text-fg-muted">{title}</div>
      <div className="overflow-hidden rounded-lg border border-border">
        <table className="w-full text-[11px]">
          <thead className="bg-panel text-fg-subtle">
            <tr>
              <th className="px-2.5 py-1.5 text-left font-normal">Name</th>
              <th className="px-2 py-1.5 text-right font-normal">Calls</th>
              <th className="px-2 py-1.5 text-right font-normal">Failed</th>
              <th className="px-2 py-1.5 text-right font-normal" title="Total time, and average per call">Time · avg</th>
              <th className="px-2 py-1.5 text-right font-normal" title="Slowest call">Max</th>
              <th className="px-2.5 py-1.5 text-right font-normal" title="Estimated from the size of the calls and their results">Tokens</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((s) => {
              const value = sort === "time" ? s.durationMs : sort === "tokens" ? s.tokens : sort === "failed" ? s.failed : s.calls;
              const rate = failRate(s);
              return (
                <tr key={s.name} className="border-t border-border">
                  <td className="relative px-2.5 py-1.5">
                    <span className="absolute inset-y-1 left-0 rounded-r bg-hover" style={{ width: `${(value * 100) / max}%` }} />
                    <span className="relative font-mono text-fg">
                      {prefix}
                      {s.name}
                    </span>
                  </td>
                  <td className="px-2 py-1.5 text-right font-mono text-fg">{s.calls.toLocaleString()}</td>
                  <td className={cn("px-2 py-1.5 text-right font-mono", s.failed ? (rate >= 10 ? "text-danger" : "text-fg") : "text-fg-subtle")}>
                    {s.failed ? `${s.failed} · ${rate < 1 ? "<1" : Math.round(rate)}%` : "–"}
                  </td>
                  <td className="px-2 py-1.5 text-right font-mono text-fg-muted">
                    {s.timed ? `${elapsed(s.durationMs)} · ${elapsed(s.durationMs / s.timed)}` : "–"}
                  </td>
                  <td className="px-2 py-1.5 text-right font-mono text-fg-subtle">{s.timed ? elapsed(s.maxMs) : "–"}</td>
                  <td className="px-2.5 py-1.5 text-right font-mono text-fg-muted">{formatTokens(s.tokens)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {rows.length > SHOWN && (
        <button onClick={() => setAll(!all)} className="text-[11px] text-fg-subtle hover:text-fg">
          {all ? "Show fewer" : `Show all ${rows.length}`}
        </button>
      )}
    </div>
  );
}

function Failures({ report }: { report: ToolReport }) {
  const [open, setOpen] = useState<number | null>(null);
  if (!report.failures.length) return null;
  return (
    <div className="space-y-1.5">
      <div className="text-[11px] font-medium text-fg-muted">
        Failed calls{report.failed > report.failures.length ? ` (latest ${report.failures.length} of ${report.failed})` : ""}
      </div>
      <div className="divide-y divide-border overflow-hidden rounded-lg border border-border">
        {report.failures.map((f, i) => (
          <button key={i} onClick={() => setOpen(open === i ? null : i)} className="block w-full px-2.5 py-1.5 text-left text-[11px] hover:bg-hover">
            <div className="flex items-baseline gap-2">
              <span className="shrink-0 font-mono text-danger">{f.tool}</span>
              <span className="min-w-0 flex-1 truncate font-mono text-fg-muted">{f.input}</span>
              {f.durationMs != null && <span className="shrink-0 font-mono text-fg-subtle">{elapsed(f.durationMs)}</span>}
              <span className="shrink-0 text-fg-subtle" title={new Date(f.at).toLocaleString()}>
                {relativeTime(f.at)}
              </span>
            </div>
            {open === i ? (
              <div className="mt-1 space-y-1">
                <pre className="whitespace-pre-wrap break-all rounded bg-panel p-2 font-mono text-[10px] text-fg-muted">{f.error || "No output."}</pre>
                <div className="text-[10px] text-fg-subtle">
                  {f.provider === "codex" ? "Codex" : "Claude Code"}
                  {f.cwd ? ` · ${f.cwd}` : ""}
                </div>
              </div>
            ) : (
              f.error && <div className="truncate text-[10px] text-fg-subtle">{f.error.split("\n").find((l) => l.trim()) ?? ""}</div>
            )}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Settings → Usage → Tools: what the agents' tool calls and shell commands cost and how often they failed. */
export function ToolStats() {
  const [range, pick] = useSaved<Range>("winfer.toolsRange", RANGES.map((r) => r.id), "24h");
  const [sort, setSort] = useSaved<ToolSort>("winfer.toolsSort", SORTS.map((s) => s.id), "calls");
  const [report, setReport] = useState<{ range: Range; report: ToolReport } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = (r: Range) => {
    setLoading(true);
    setError(null);
    fetchToolStats(RANGES.find((x) => x.id === r)?.days ?? 1)
      .then((rep) => setReport({ range: r, report: rep }), (e) => setError(String(e)))
      .finally(() => setLoading(false));
  };
  useEffect(() => load(range), [range]);

  const rep = report?.report;
  const rate = rep ? failRate(rep) : 0;

  return (
    <div className="space-y-4">
      <p className="text-xs text-fg-muted">
        Tool calls and shell commands your agents ran, read from Claude Code and Codex's logs: how often, how many failed, how long they took and roughly how many
        tokens their input and output added to the context.
      </p>

      <div className="flex items-center gap-2">
        <div className="flex rounded-md border border-border p-0.5">
          {RANGES.map((r) => (
            <button key={r.id} onClick={() => pick(r.id)} className={cn("rounded px-2.5 py-1 text-xs", range === r.id ? "bg-active text-fg" : "text-fg-muted hover:text-fg")}>
              {r.label}
            </button>
          ))}
        </div>
        <div className="flex-1" />
        <Segmented value={sort} options={SORTS} onChange={setSort} />
        <button onClick={() => load(range)} disabled={loading} title="Read the logs again" className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-fg disabled:opacity-50">
          {loading ? <Spinner size={13} /> : <RefreshCw size={13} />}
        </button>
      </div>

      {error && <p className="text-xs text-danger">{error}</p>}
      {!rep && !error && (
        <div className="flex items-center gap-1.5 text-xs text-fg-subtle">
          <Spinner size={12} /> Reading tool calls from the agents' logs…
        </div>
      )}

      {rep && (
        <div className={cn("space-y-4 transition-opacity", report?.range !== range && "opacity-60")}>
          <div className="flex gap-2">
            <Stat label="Tool calls" value={rep.calls.toLocaleString()} detail={`${rep.sessions.toLocaleString()} sessions`} />
            <Stat label="Failed" value={rep.failed.toLocaleString()} detail={rep.calls ? `${rate < 1 && rep.failed ? "<1" : Math.round(rate)}% of calls` : undefined} />
            <Stat label="Time in tools" value={elapsed(rep.durationMs)} detail={rep.calls ? `${elapsed(rep.durationMs / rep.calls)} per call` : undefined} />
            <Stat label="Tokens (est.)" value={formatTokens(rep.tokens)} detail="input and results" />
          </div>
          {rep.calls === 0 ? (
            <p className="text-xs text-fg-subtle">No tool calls in this range.</p>
          ) : (
            <>
              <StatTable title="Tools" stats={rep.tools} sort={sort} />
              <StatTable title="Shell commands" stats={rep.commands} sort={sort} prefix="$ " />
              <Failures report={rep} />
            </>
          )}
        </div>
      )}
    </div>
  );
}
