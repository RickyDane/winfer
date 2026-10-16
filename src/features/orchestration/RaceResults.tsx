import { BarChart3, RefreshCw, X } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { StatusDot } from "@/features/terminal/StatusDot";
import { formatCost, formatTokens } from "@/features/usage/history";
import { refreshTaskCosts, type DirUsage } from "@/features/usage/taskCost";
import { worktreeDiff } from "@/features/worktree/worktree";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { closeRaceResults, useRaceResults } from "./resultsStore";
import { activeMs, useRunTimes } from "./runTimes";

interface Changes {
  files: number;
  adds: number;
  dels: number;
  ahead: number;
  dirty: boolean;
}

/** Files and lines changed since the branch point, new (untracked) files included. */
async function changesOf(pane: Pane): Promise<Changes> {
  const d = await worktreeDiff(pane.worktree!);
  let files = d.untracked.length, adds = d.untrackedLines, dels = 0;
  for (const line of d.diff.split("\n")) {
    if (line.startsWith("diff --git ")) files++;
    else if (line.startsWith("+") && !line.startsWith("+++")) adds++;
    else if (line.startsWith("-") && !line.startsWith("---")) dels++;
  }
  return { files, adds, dels, ahead: d.ahead, dirty: d.dirty };
}

/** One measurement per pane; null where it's unknown. */
interface Metric {
  label: string;
  hint?: string;
  values: (number | null)[];
  format: (n: number) => string;
  /** Which way is better, for highlighting the winner and coloring the differences. */
  better?: "lower" | "higher";
}

function sumModels(usage: DirUsage | undefined, key: "input" | "output" | "cacheRead" | "cacheWrite"): number | null {
  return usage ? usage.models.reduce((n, m) => n + m[key], 0) : null;
}

const count = (n: number) => n.toLocaleString("en-US");
/** "45s", "3m 20s", "1h 05m": seconds matter when comparing short runs. */
function ms(n: number): string {
  const sec = Math.round(n / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ${String(sec % 60).padStart(2, "0")}s`;
  return `${Math.floor(min / 60)}h ${String(min % 60).padStart(2, "0")}m`;
}

/** "−42%" against the first pane's value, colored by whether that's better. */
function Delta({ value, base, better }: { value: number; base: number; better?: Metric["better"] }) {
  if (base === 0 || value === base) return null;
  const pct = ((value - base) / base) * 100;
  const good = better && (better === "lower" ? pct < 0 : pct > 0);
  return (
    <span className={cn("ml-1.5 text-[10px]", !better ? "text-fg-subtle" : good ? "text-success" : "text-danger")}>
      {pct > 0 ? "+" : "−"}
      {Math.abs(pct) >= 10 ? Math.round(Math.abs(pct)) : Math.abs(pct).toFixed(1)}%
    </span>
  );
}

/** Side-by-side numbers for a race or A/B test: time, tokens, cost and code written. */
function Results({ raceId, panes }: { raceId: string; panes: Pane[] }) {
  const providers = useWorkspaces((s) => s.providers);
  const times = useRunTimes((s) => s.byPane);
  const [usage, setUsage] = useState<Record<string, DirUsage>>({});
  const [changes, setChanges] = useState<Record<string, Changes | string>>({});
  const [loading, setLoading] = useState(false);
  const [now, setNow] = useState(Date.now());

  const load = useCallback(async () => {
    setLoading(true);
    setNow(Date.now());
    const withTree = panes.filter((p) => p.worktree);
    const [found] = await Promise.all([
      refreshTaskCosts(withTree.map((p) => p.worktree!.path)),
      ...withTree.map((p) =>
        changesOf(p).then(
          (c) => setChanges((s) => ({ ...s, [p.id]: c })),
          (e) => setChanges((s) => ({ ...s, [p.id]: String(e) })),
        ),
      ),
    ]);
    const norm = (d: string) => d.replace(/\\/g, "/").replace(/(.)\/+$/, "$1");
    setUsage(Object.fromEntries(withTree.flatMap((p) => {
      const u = found.find((d) => norm(d.dir) === norm(p.worktree!.path));
      return u ? [[p.id, u]] : [];
    })));
    setLoading(false);
  }, [panes]);

  // Reload as worktrees appear and as agents start and finish.
  const key = panes.map((p) => `${p.worktree?.path ?? ""}:${p.status}`).join("|");
  useEffect(() => void load(), [raceId, key]); // eslint-disable-line react-hooks/exhaustive-deps

  const ab = raceId.startsWith("ab-");
  const u = (p: Pane) => usage[p.id];
  const c = (p: Pane) => (typeof changes[p.id] === "object" ? (changes[p.id] as Changes) : undefined);
  // After a restart there are no status timings: the first and last model call stand in.
  const active = (p: Pane) => (times[p.id] ? activeMs(times[p.id], now) : null);
  const wall = (p: Pane) => {
    const t = times[p.id];
    if (t) return (t.since !== null ? now : t.finishedAt ?? now) - t.startedAt;
    const usage = u(p);
    return usage?.firstAt != null && usage.lastAt != null ? usage.lastAt - usage.firstAt : null;
  };

  const metrics: Metric[] = [
    { label: "Working time", hint: "Time the agent spent working, without waits for input", values: panes.map(active), format: ms, better: "lower" },
    { label: "Run time", hint: "From its first start to when it last finished", values: panes.map(wall), format: ms, better: "lower" },
    { label: "Cost", hint: "At API prices, from the agents' usage logs", values: panes.map((p) => u(p)?.cost ?? null), format: formatCost, better: "lower" },
    { label: "Tokens", values: panes.map((p) => u(p)?.tokens ?? null), format: formatTokens, better: "lower" },
    { label: "· Input", values: panes.map((p) => sumModels(u(p), "input")), format: formatTokens, better: "lower" },
    { label: "· Output", values: panes.map((p) => sumModels(u(p), "output")), format: formatTokens, better: "lower" },
    { label: "· Cache read", values: panes.map((p) => sumModels(u(p), "cacheRead")), format: formatTokens },
    { label: "· Cache write", values: panes.map((p) => sumModels(u(p), "cacheWrite")), format: formatTokens },
    { label: "Model calls", values: panes.map((p) => u(p)?.calls ?? null), format: count, better: "lower" },
    { label: "Lines written", hint: "Added lines, new files included", values: panes.map((p) => c(p)?.adds ?? null), format: count },
    { label: "Lines removed", values: panes.map((p) => c(p)?.dels ?? null), format: count },
    { label: "Files changed", values: panes.map((p) => c(p)?.files ?? null), format: count },
    { label: "Commits", values: panes.map((p) => c(p)?.ahead ?? null), format: count },
    {
      label: "Cost per 100 lines",
      values: panes.map((p) => {
        const cost = u(p)?.cost, lines = c(p)?.adds;
        return cost !== undefined && lines ? (cost / lines) * 100 : null;
      }),
      format: formatCost,
      better: "lower",
    },
  ];

  const bestOf = (m: Metric): number | null => {
    if (!m.better) return null;
    const known = m.values.filter((v): v is number => v !== null);
    if (known.length < 2 || known.every((v) => v === known[0])) return null;
    return m.better === "lower" ? Math.min(...known) : Math.max(...known);
  };

  const label = (p: Pane) => p.customTitle ?? providerName(providers, p.provider);
  const models = (p: Pane) => [...new Set(u(p)?.models.map((m) => m.model) ?? [])].join(", ") || p.model || "default";

  return (
    <div className="space-y-4">
      <p className="truncate text-xs text-fg-muted" title={panes[0]?.prompt ?? undefined}>
        {panes[0]?.prompt ?? (ab ? "A/B test" : "Race")}
      </p>
      <div className="overflow-x-auto rounded-lg border border-border">
        <table className="w-full text-xs">
          <thead>
            <tr className="border-b border-border bg-panel">
              <th className="w-40 px-3 py-2 text-left font-medium text-fg-subtle">
                <button onClick={() => void load()} disabled={loading} className="rounded p-1 hover:bg-hover hover:text-fg" title="Refresh">
                  <RefreshCw size={11} className={cn(loading && "animate-spin")} />
                </button>
              </th>
              {panes.map((p) => (
                <th key={p.id} className="px-3 py-2 text-left font-medium text-fg">
                  <div className="flex items-center gap-1.5">
                    <StatusDot status={p.status} />
                    <ProviderIcon provider={p.provider} size={12} />
                    <span className="truncate">{label(p)}</span>
                  </div>
                  <div className="mt-0.5 flex flex-wrap gap-1 text-[10px] font-normal">
                    {ab && (
                      <span className={cn("rounded px-1", p.plain ? "bg-hover text-fg-muted" : "bg-accent-soft text-accent")}>
                        {p.plain ? "plain" : "with tools"}
                      </span>
                    )}
                    <span className="truncate font-mono text-fg-subtle">{models(p)}</span>
                  </div>
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {metrics.map((m) => {
              const best = bestOf(m);
              const base = m.values[0];
              return (
                <tr key={m.label}>
                  <td className={cn("px-3 py-1.5", m.label.startsWith("·") ? "pl-5 text-fg-subtle" : "text-fg-muted")} title={m.hint}>
                    {m.label.replace(/^· /, "")}
                  </td>
                  {m.values.map((v, i) => (
                    <td key={panes[i].id} className={cn("px-3 py-1.5 font-mono", v !== null && v === best ? "font-semibold text-success" : "text-fg")}>
                      {v === null ? <span className="text-fg-subtle">—</span> : m.format(v)}
                      {i > 0 && v !== null && base !== null && <Delta value={v} base={base} better={m.better} />}
                    </td>
                  ))}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {ab && <Summary panes={panes} metrics={metrics} />}
      {panes.some((p) => typeof changes[p.id] === "string") && (
        <p className="text-[11px] text-danger">{panes.map((p) => changes[p.id]).filter((x) => typeof x === "string").join(" · ")}</p>
      )}
      <p className="text-[11px] leading-relaxed text-fg-subtle">
        Cost is at API prices, read from the agents' usage logs per worktree; it can lag a few seconds behind. Working time is measured while winfer runs.
        {ab && " The plain run started without winfer's tools, the browser, and your own MCP servers and skills."}
      </p>
    </div>
  );
}

/** A sentence per headline number for the plain run against the one with tools. */
function Summary({ panes, metrics }: { panes: Pane[]; metrics: Metric[] }) {
  const a = panes.findIndex((p) => !p.plain);
  const b = panes.findIndex((p) => p.plain);
  if (a < 0 || b < 0) return null;
  const lines: ReactNode[] = [];
  const phrases: [string, string, string][] = [["Cost", "cost", ""], ["Tokens", "used", " tokens"], ["Working time", "took", ""], ["Lines written", "wrote", " lines"]];
  for (const [label, verb, unit] of phrases) {
    const m = metrics.find((x) => x.label === label)!;
    const [va, vb] = [m.values[a], m.values[b]];
    if (va === null || vb === null || va === vb) continue;
    const more = va > vb;
    lines.push(
      <li key={label}>
        <span className="text-fg">With tools</span> {verb} <span className="font-mono text-fg">{m.format(Math.abs(va - vb))}</span> {more ? "more" : "less"}
        {unit}
        {vb > 0 && <span className="text-fg-subtle"> ({more ? "+" : "−"}{Math.round((Math.abs(va - vb) / vb) * 100)}% vs. plain)</span>}
      </li>,
    );
  }
  if (!lines.length) return null;
  return (
    <ul className="list-inside list-disc space-y-0.5 rounded-lg border border-border bg-panel px-3 py-2 text-xs text-fg-muted">{lines}</ul>
  );
}

/** Modal with the detailed numbers of a finished race or A/B test. */
export function RaceResults() {
  const raceId = useRaceResults((s) => s.raceId);
  const workspaces = useWorkspaces((s) => s.workspaces);
  const panes = raceId ? workspaces.flatMap((w) => w.panes).filter((p) => p.raceId === raceId) : [];

  if (!raceId) return null;
  const ab = raceId.startsWith("ab-");
  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex justify-center bg-black/40 pt-[8vh]" onMouseDown={closeRaceResults}>
      <div className="anim-dialog h-fit max-h-[84vh] w-[min(860px,calc(100vw-32px))] space-y-4 overflow-y-auto rounded-xl border border-border-strong bg-panel-raised p-5 shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-2">
          <BarChart3 size={14} className="text-accent" />
          <h1 className="flex-1 text-sm font-semibold text-fg">{ab ? "A/B test results" : "Race results"}</h1>
          <button onClick={closeRaceResults} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>
        {panes.length ? <Results raceId={raceId} panes={panes} /> : <p className="text-xs text-fg-muted">These agents have been closed.</p>}
      </div>
    </div>
  );
}
