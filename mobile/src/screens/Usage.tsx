import { useCallback, useEffect, useId, useMemo, useRef, useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { go, haptic, useTick } from "../hooks";
import type { UsageAccount, UsageModelTotal, UsageProvider, UsageRange, UsageTokens, UsageWindow } from "../protocol";
import { Icon, ProviderMark, providerInfo } from "./common";
import { usePullToRefresh } from "./widgets";

const STALE_MS = 600_000;
const RANGES: { id: UsageRange; label: string; title: string }[] = [
  { id: "24h", label: "24h", title: "Last 24 hours" },
  { id: "7d", label: "7d", title: "Last 7 days" },
  { id: "30d", label: "30d", title: "Last 30 days" },
  { id: "90d", label: "90d", title: "Last 90 days" },
];

type Tab = "cost" | "tokens" | "limits";
type LimitStyle = "bars" | "lines";

/** Meter color: calm green while plenty is left, through amber to red. */
const level = (left: number) =>
  left <= 10 ? "var(--danger)" : left < 30 ? "var(--warn)" : left < 60 ? "color-mix(in srgb, var(--warn) 45%, var(--ok))" : "var(--ok)";

const toneOf = (w: UsageWindow): "danger" | "warn" | undefined =>
  w.left <= 10 ? "danger" : w.runsOutAt || w.left < 25 ? "warn" : undefined;

function dur(ms: number): string {
  const m = Math.max(1, Math.round(ms / 60_000));
  const [d, h, mm] = [Math.floor(m / 1440), Math.floor((m % 1440) / 60), m % 60];
  if (d) return h ? `${d}d ${h}h` : `${d}d`;
  if (h) return mm ? `${h}h ${mm}m` : `${h}h`;
  return `${mm}m`;
}

const clock = (ms: number) => new Date(ms).toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit" });

function fmtTokens(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0";
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(n >= 10_000_000_000 ? 1 : 2)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 100_000_000 ? 0 : n >= 10_000_000 ? 1 : 2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 100_000 ? 0 : 1)}k`;
  return String(Math.round(n));
}

function fmtCost(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "$0";
  if (n < 0.01) return "<$0.01";
  if (n >= 100) return `$${n.toFixed(0)}`;
  return `$${n.toFixed(2)}`;
}

function fmtBucketTime(ms: number, range: UsageRange): string {
  const d = new Date(ms);
  if (range === "24h") return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (range === "7d") return d.toLocaleString([], { weekday: "short", hour: "2-digit" });
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

function freshness(at: number): { stale: boolean; short: string } {
  const a = Math.max(0, Date.now() - at);
  return { stale: a > STALE_MS, short: a < 120_000 ? "just now" : `${dur(a)} ago` };
}

function windowSpanMs(kind: string): number | null {
  if (kind === "month-credit") return 30 * 86_400_000;
  if (kind === "session" || kind.startsWith("session-")) return 5 * 3_600_000;
  if (kind === "week" || kind.startsWith("week-")) return 7 * 86_400_000;
  return null;
}

function LimitLineChart({ w }: { w: UsageWindow }) {
  const now = Date.now();
  const left = Math.max(0, Math.min(100, w.left));
  const live = Boolean(w.resetsAt && w.resetsAt > now);
  const span = windowSpanMs(w.kind);
  const raw = (w.samples ?? []).filter(([t]) => t <= now).sort((a, b) => a[0] - b[0]);
  const winStart = live && span && w.resetsAt ? w.resetsAt - span : null;
  const start = Math.min(
    winStart ?? Infinity,
    raw.length ? raw[0][0] : Infinity,
    now - 3_600_000,
  );
  const end = live && w.resetsAt ? w.resetsAt : now;
  const range = Math.max(1, end - start);

  const pts: [number, number][] = [];
  if (winStart !== null && (!raw.length || raw[0][0] > winStart)) {
    pts.push([winStart, 100]);
  }
  for (const p of raw) pts.push(p);
  if (!pts.length) pts.push([start, left]);
  if (pts[pts.length - 1][0] < now) pts.push([now, left]);

  const W = 300;
  const H = 56;
  const padY = 5;
  const xOf = (t: number) => Math.max(0, Math.min(W, ((t - start) / range) * W));
  const yOf = (pct: number) => padY + (1 - Math.max(0, Math.min(100, pct)) / 100) * (H - padY * 2);

  const lineD = pts.map(([t, v], i) => `${i === 0 ? "M" : "L"}${xOf(t).toFixed(1)},${yOf(v).toFixed(1)}`).join(" ");
  const lastX = xOf(now);
  const lastY = yOf(left);
  const firstX = xOf(pts[0][0]);
  const areaD = `${lineD} L${lastX.toFixed(1)},${(H - padY).toFixed(1)} L${firstX.toFixed(1)},${(H - padY).toFixed(1)} Z`;

  const runsOut = w.runsOutAt && w.runsOutAt > now ? w.runsOutAt : null;
  let projEnd: { x: number; y: number; warn: boolean } | null = null;
  if (live && end > now) {
    if (runsOut) {
      projEnd = { x: xOf(Math.min(runsOut, end)), y: yOf(0), warn: true };
    } else if (now > start && left < 100) {
      const rate = (100 - left) / (now - start);
      const endLeft = Math.max(0, left - rate * (end - now));
      projEnd = { x: xOf(end), y: yOf(endLeft), warn: false };
    } else {
      projEnd = { x: xOf(end), y: yOf(left), warn: false };
    }
  }

  const col = left < 40 ? level(left) : "var(--accent)";
  const gradId = `ug-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;

  return (
    <div className="usage-line" role="img" aria-label={`${w.label}: ${Math.round(left)}% remaining`}>
      <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none">
        <defs>
          <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={col} stopOpacity="0.22" />
            <stop offset="100%" stopColor={col} stopOpacity="0" />
          </linearGradient>
        </defs>
        <line className="usage-line-grid" x1={0} y1={yOf(50)} x2={W} y2={yOf(50)} />
        <line className="usage-line-grid" x1={0} y1={yOf(0)} x2={W} y2={yOf(0)} />
        {live && <line className="usage-line-now" x1={lastX} y1={padY} x2={lastX} y2={H - padY} />}
        <path d={areaD} fill={`url(#${gradId})`} />
        <path d={lineD} fill="none" stroke={col} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
        {projEnd && (
          <line
            x1={lastX}
            y1={lastY}
            x2={projEnd.x}
            y2={projEnd.y}
            stroke={projEnd.warn ? "var(--warn)" : "var(--subtle)"}
            strokeWidth={1.75}
            strokeDasharray="4 4"
            strokeLinecap="round"
            vectorEffect="non-scaling-stroke"
          />
        )}
        <circle cx={lastX} cy={lastY} r={3.2} fill={col} />
      </svg>
    </div>
  );
}

function WindowTile({ w, style }: { w: UsageWindow; style: LimitStyle }) {
  const now = Date.now();
  const pct = Math.max(0, Math.min(100, Math.round(w.left)));
  const reset = w.resetsAt && w.resetsAt > now ? w.resetsAt : null;
  const runsOut = w.runsOutAt && w.runsOutAt > now ? w.runsOutAt : null;
  const tone = toneOf(w);
  const col = level(w.left);

  return (
    <div className="usage-tile" data-tone={tone}>
      <div className="usage-tile-head">
        <b>{w.label}</b>
        <span className="num" style={{ color: col }}>{pct}% left</span>
      </div>
      {style === "bars" ? (
        <div className="usage-bar-track" role="meter" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} style={{ ["--lvl" as string]: col }}>
          <i style={{ width: `${Math.max(pct, pct > 0 ? 2 : 0)}%` }} />
        </div>
      ) : (
        <LimitLineChart w={w} />
      )}
      <div className="usage-tile-detail">
        {reset ? (
          <span>Resets {clock(reset)} · in {dur(reset - now)}</span>
        ) : (
          <span>{w.left >= 99.5 ? "Full" : "Available"}</span>
        )}
        {runsOut && <span className="warn">At current pace, runs out {clock(runsOut)}</span>}
      </div>
    </div>
  );
}

function AccountRow({ acc }: { acc: UsageAccount }) {
  const f = freshness(acc.at);
  return (
    <div className="usage-acct-row">
      <div className="usage-acct-top">
        <b>{acc.label}</b>
        {f.stale && <span className="usage-stale-tag">Stale · {f.short}</span>}
      </div>
      <div className="usage-acct-pills">
        {acc.windows.map((w) => {
          const pct = Math.max(0, Math.min(100, Math.round(w.left)));
          return (
            <div className="usage-mini-pill" key={w.kind + w.label} style={{ ["--lvl" as string]: level(w.left) }}>
              <div className="usage-mini-top">
                <span>{w.label}</span>
                <b className="num">{pct}%</b>
              </div>
              <div className="usage-mini-bar">
                <i style={{ width: `${Math.max(pct, pct > 0 ? 3 : 0)}%` }} />
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function ProviderCard({ p, index, style }: { p: UsageProvider; index: number; style: LimitStyle }) {
  const [open, setOpen] = useState(false);
  const multi = p.accounts.length > 1;
  const main = multi && p.pooled ? p.pooled : p.accounts[0];
  const f = freshness(main.at);
  const info = providerInfo(p.id);
  const brand = info?.mark ?? "var(--accent)";
  const accountAlert = multi && p.accounts.some((a) => freshness(a.at).stale || a.windows.some((w) => toneOf(w)));

  return (
    <section className="usage-card" style={{ ["--i" as string]: index } as CSSProperties}>
      <div className="usage-card-head">
        <span className="usage-brand" style={{ color: brand, ["--brand" as string]: brand } as CSSProperties}>
          <ProviderMark provider={p.id} size={16} />
        </span>
        <div className="usage-card-title">
          <b>{p.name}</b>
          {!multi && p.accounts[0].key.includes(":") && <span>{p.accounts[0].label}</span>}
        </div>
        {f.stale && <span className="usage-stale-tag">Stale · {f.short}</span>}
      </div>

      <div className="usage-grid">
        {main.windows.map((w) => (
          <WindowTile key={w.kind + w.label} w={w} style={style} />
        ))}
      </div>

      {multi && (
        <div className="usage-drawer-wrap">
          <button
            type="button"
            className="usage-drawer-btn"
            aria-expanded={open}
            onClick={() => {
              haptic(5);
              setOpen((v) => !v);
            }}
          >
            <span className="usage-drawer-label">
              <b>{p.accounts.length} accounts</b>
              <span>{p.accounts.map((a) => a.label).join(" · ")}</span>
            </span>
            {accountAlert && <span className="usage-alert-dot" aria-label="Attention needed" />}
            <Icon name="chevron" size={15} className="usage-chev" />
          </button>
          <div className="usage-drawer" data-open={open || undefined}>
            <div className="usage-drawer-inner">
              {p.accounts.map((acc) => (
                <AccountRow key={acc.key} acc={acc} />
              ))}
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

function ModelRow({ m, maxVal, mode }: { m: UsageModelTotal; maxVal: number; mode: "cost" | "tokens" }) {
  const [open, setOpen] = useState(false);
  const total = m.input + m.output + m.cacheRead + m.cacheWrite;
  const val = mode === "cost" ? m.cost : total;
  const pct = maxVal > 0 ? Math.max(3, (val / maxVal) * 100) : 0;
  const cachePct = total > 0 ? Math.round((m.cacheRead / total) * 100) : 0;
  const brand = providerInfo(m.provider)?.mark ?? "var(--accent)";
  return (
    <div className="usage-model-wrap">
      <button
        type="button"
        className="usage-model-row"
        aria-expanded={open}
        onClick={() => {
          haptic(4);
          setOpen((v) => !v);
        }}
      >
        <span className="usage-model-dot" style={{ background: brand }} aria-hidden />
        <div className="usage-model-main">
          <div className="usage-model-top">
            <b>{m.model}</b>
            <span className="usage-model-vals num">
              {mode === "cost" ? (
                <>
                  <b>{m.priced ? fmtCost(m.cost) : "—"}</b>
                  <em>{fmtTokens(total)}</em>
                </>
              ) : (
                <>
                  <b>{fmtTokens(total)}</b>
                  {cachePct > 0 && <em>{cachePct}% cached</em>}
                </>
              )}
            </span>
          </div>
          <div className="usage-mini-bar">
            <i style={{ width: `${pct}%`, background: brand }} />
          </div>
        </div>
      </button>
      {open && (
        <div className="usage-model-detail num">
          <span><em>Input</em> {fmtTokens(m.input)}</span>
          <span><em>Output</em> {fmtTokens(m.output)}</span>
          <span><em>Cache read</em> {fmtTokens(m.cacheRead)}</span>
          <span><em>Cache write</em> {fmtTokens(m.cacheWrite)}</span>
        </div>
      )}
    </div>
  );
}

function RangeBar({ range, onRange }: { range: UsageRange; onRange: (r: UsageRange) => void }) {
  const rangeMeta = RANGES.find((r) => r.id === range) ?? RANGES[1];
  return (
    <div className="usage-range-bar">
      <span>{rangeMeta.title}</span>
      <div className="usage-range-pills" role="group" aria-label="Time range">
        {RANGES.map((r) => (
          <button
            key={r.id}
            type="button"
            aria-pressed={r.id === range}
            onClick={() => {
              haptic(5);
              onRange(r.id);
            }}
          >
            {r.label}
          </button>
        ))}
      </div>
    </div>
  );
}

function CostView({
  tokens,
  range,
  onRange,
}: {
  tokens: UsageTokens | null | undefined;
  range: UsageRange;
  onRange: (r: UsageRange) => void;
}) {
  const [selected, setSelected] = useState<number | null>(null);
  useEffect(() => setSelected(null), [range]);

  if (!tokens) {
    return (
      <div className="usage-empty">
        <Icon name="gauge" size={26} />
        <b>Cost history unavailable</b>
        <p>Update winfer on your desktop to view estimated API spend here.</p>
      </div>
    );
  }

  const rangeMeta = RANGES.find((r) => r.id === range) ?? RANGES[1];
  const avgDivisor = range === "24h" ? 24 : range === "30d" ? 30 : range === "90d" ? 90 : 7;
  const avgLabel = range === "24h" ? "Avg / hour" : "Avg / day";
  const avgCost = tokens.cost / avgDivisor;
  const maxBucket = Math.max(...tokens.buckets.map((b) => b.cost), 0.0001);
  const activeBucket = selected !== null ? tokens.buckets[selected] : null;
  const modelsByCost = [...tokens.models].sort((a, b) => b.cost - a.cost);
  const maxModel = Math.max(...modelsByCost.map((m) => m.cost), 0.0001);
  const providersByCost = [...tokens.providers].filter((p) => p.cost > 0).sort((a, b) => b.cost - a.cost);

  return (
    <>
      <RangeBar range={range} onRange={onRange} />

      {tokens.cost <= 0 ? (
        <div className="usage-empty">
          <Icon name="gauge" size={26} />
          <b>No spend in {rangeMeta.label}</b>
          <p>Switch to a wider range or run an agent session to see estimated API cost here.</p>
        </div>
      ) : (
        <>
          <section className="usage-card" style={{ ["--i" as string]: 0 } as CSSProperties}>
            <div className="usage-kpis">
              <div className="usage-kpi">
                <span>Est. API cost</span>
                <b className="num">{fmtCost(tokens.cost)}</b>
                <em>{modelsByCost.length} {modelsByCost.length === 1 ? "model" : "models"}</em>
              </div>
              <div className="usage-kpi-sep" aria-hidden />
              <div className="usage-kpi">
                <span>{avgLabel}</span>
                <b className="num">{fmtCost(avgCost)}</b>
                <em>{fmtTokens(tokens.input + tokens.output + tokens.cacheRead + tokens.cacheWrite)} tokens</em>
              </div>
            </div>

            {providersByCost.length > 0 && (
              <div className="usage-comp">
                <div className="usage-comp-bar" aria-hidden>
                  {providersByCost.map((pr) => (
                    <i
                      key={pr.id}
                      style={{
                        width: `${Math.max(2, (pr.cost / tokens.cost) * 100)}%`,
                        background: providerInfo(pr.id)?.mark ?? "var(--accent)",
                      }}
                    />
                  ))}
                </div>
                <div className="usage-comp-grid">
                  {providersByCost.map((pr) => {
                    const pct = Math.round((pr.cost / tokens.cost) * 100);
                    return (
                      <div className="usage-comp-item" key={pr.id}>
                        <i style={{ background: providerInfo(pr.id)?.mark ?? "var(--accent)" }} aria-hidden />
                        <span>{pr.name}</span>
                        <b className="num">{fmtCost(pr.cost)}</b>
                        <em className="num">{pct}%</em>
                      </div>
                    );
                  })}
                </div>
              </div>
            )}
          </section>

          {tokens.buckets.length > 0 && (
            <section className="usage-card" style={{ ["--i" as string]: 1 } as CSSProperties}>
              <div className="usage-sec-head">
                <b>Spend over time</b>
                <span className="num">
                  {activeBucket
                    ? `${fmtBucketTime(activeBucket.start, range)} · ${fmtCost(activeBucket.cost)} · ${fmtTokens(activeBucket.tokens)}`
                    : `Peak ${fmtCost(maxBucket)}`}
                </span>
              </div>
              <div className="usage-chart" data-dense={tokens.buckets.length > 40 || undefined} role="img" aria-label="Estimated cost over time">
                {tokens.buckets.map((b, i) => {
                  const h = b.cost > 0 ? Math.max(6, Math.round((b.cost / maxBucket) * 100)) : 0;
                  return (
                    <button
                      key={b.start}
                      type="button"
                      className="usage-bar-col"
                      data-active={selected === i || undefined}
                      aria-label={`${fmtBucketTime(b.start, range)}: ${fmtCost(b.cost)}`}
                      onClick={() => {
                        haptic(3);
                        setSelected((cur) => (cur === i ? null : i));
                      }}
                    >
                      <span style={{ height: `${h}%` }} />
                    </button>
                  );
                })}
              </div>
              <div className="usage-chart-axis">
                <span>{fmtBucketTime(tokens.buckets[0].start, range)}</span>
                <span>Now</span>
              </div>
            </section>
          )}

          {modelsByCost.length > 0 && (
            <section className="usage-card" style={{ ["--i" as string]: 2 } as CSSProperties}>
              <div className="usage-sec-head">
                <b>Cost by model</b>
                <span>Tap for breakdown</span>
              </div>
              <div className="usage-models">
                {modelsByCost.map((m) => (
                  <ModelRow key={`${m.provider}:${m.model}`} m={m} maxVal={maxModel} mode="cost" />
                ))}
              </div>
            </section>
          )}
        </>
      )}
    </>
  );
}

function TokensView({
  tokens,
  range,
  onRange,
}: {
  tokens: UsageTokens | null | undefined;
  range: UsageRange;
  onRange: (r: UsageRange) => void;
}) {
  const [selected, setSelected] = useState<number | null>(null);
  useEffect(() => setSelected(null), [range]);

  const rangeMeta = RANGES.find((r) => r.id === range) ?? RANGES[1];

  if (!tokens) {
    return (
      <div className="usage-empty">
        <Icon name="gauge" size={26} />
        <b>Token history unavailable</b>
        <p>Update winfer on your desktop to view detailed token usage here.</p>
      </div>
    );
  }

  const totalTokens = tokens.input + tokens.output + tokens.cacheRead + tokens.cacheWrite;
  const cachePct = totalTokens > 0 ? Math.round((tokens.cacheRead / totalTokens) * 100) : 0;
  const maxBucket = Math.max(...tokens.buckets.map((b) => b.tokens), 1);
  const modelsByTokens = [...tokens.models].sort(
    (a, b) => (b.input + b.output + b.cacheRead + b.cacheWrite) - (a.input + a.output + a.cacheRead + a.cacheWrite),
  );
  const maxModel = Math.max(...modelsByTokens.map((m) => m.input + m.output + m.cacheRead + m.cacheWrite), 1);
  const activeBucket = selected !== null ? tokens.buckets[selected] : null;

  const segments = [
    { key: "in", label: "Input", value: tokens.input, color: "var(--accent)" },
    { key: "out", label: "Output", value: tokens.output, color: "var(--ok)" },
    { key: "cr", label: "Cache read", value: tokens.cacheRead, color: "color-mix(in srgb, var(--accent) 45%, var(--subtle))" },
    { key: "cw", label: "Cache write", value: tokens.cacheWrite, color: "var(--warn)" },
  ].filter((s) => s.value > 0);

  return (
    <>
      <RangeBar range={range} onRange={onRange} />

      {totalTokens === 0 ? (
        <div className="usage-empty">
          <Icon name="gauge" size={26} />
          <b>No token usage in {rangeMeta.label}</b>
          <p>Switch to a wider range or run an agent session to see token stats here.</p>
        </div>
      ) : (
        <>
          <section className="usage-card" style={{ ["--i" as string]: 0 } as CSSProperties}>
            <div className="usage-kpis">
              <div className="usage-kpi">
                <span>Total tokens</span>
                <b className="num">{fmtTokens(totalTokens)}</b>
                <em>{fmtTokens(tokens.input + tokens.output)} fresh</em>
              </div>
              <div className="usage-kpi-sep" aria-hidden />
              <div className="usage-kpi">
                <span>Cache hit</span>
                <b className="num">{cachePct}%</b>
                <em>{fmtTokens(tokens.cacheRead)} read</em>
              </div>
            </div>

            {segments.length > 0 && (
              <div className="usage-comp">
                <div className="usage-comp-bar" aria-hidden>
                  {segments.map((s) => (
                    <i key={s.key} style={{ width: `${Math.max(2, (s.value / totalTokens) * 100)}%`, background: s.color }} />
                  ))}
                </div>
                <div className="usage-comp-grid">
                  {segments.map((s) => (
                    <div className="usage-comp-item" key={s.key}>
                      <i style={{ background: s.color }} aria-hidden />
                      <span>{s.label}</span>
                      <b className="num">{fmtTokens(s.value)}</b>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </section>

          {tokens.buckets.length > 0 && (
            <section className="usage-card" style={{ ["--i" as string]: 1 } as CSSProperties}>
              <div className="usage-sec-head">
                <b>Token activity</b>
                <span className="num">
                  {activeBucket
                    ? `${fmtBucketTime(activeBucket.start, range)} · ${fmtTokens(activeBucket.tokens)}`
                    : `Peak ${fmtTokens(maxBucket)}`}
                </span>
              </div>
              <div className="usage-chart" data-dense={tokens.buckets.length > 40 || undefined} role="img" aria-label="Token usage over time">
                {tokens.buckets.map((b, i) => {
                  const h = b.tokens > 0 ? Math.max(6, Math.round((b.tokens / maxBucket) * 100)) : 0;
                  return (
                    <button
                      key={b.start}
                      type="button"
                      className="usage-bar-col"
                      data-active={selected === i || undefined}
                      aria-label={`${fmtBucketTime(b.start, range)}: ${fmtTokens(b.tokens)} tokens`}
                      onClick={() => {
                        haptic(3);
                        setSelected((cur) => (cur === i ? null : i));
                      }}
                    >
                      <span style={{ height: `${h}%` }} />
                    </button>
                  );
                })}
              </div>
              <div className="usage-chart-axis">
                <span>{fmtBucketTime(tokens.buckets[0].start, range)}</span>
                <span>Now</span>
              </div>
            </section>
          )}

          {modelsByTokens.length > 0 && (
            <section className="usage-card" style={{ ["--i" as string]: 2 } as CSSProperties}>
              <div className="usage-sec-head">
                <b>Tokens by model</b>
                <span>Tap for breakdown</span>
              </div>

              {tokens.providers.length > 1 && (
                <div className="usage-agent-share">
                  {tokens.providers.map((pr) => {
                    const pct = totalTokens > 0 ? Math.round((pr.tokens / totalTokens) * 100) : 0;
                    const brand = providerInfo(pr.id)?.mark ?? "var(--accent)";
                    return (
                      <span className="usage-agent-chip" key={pr.id}>
                        <i style={{ background: brand }} aria-hidden />
                        {pr.name}
                        <b className="num">{pct}%</b>
                      </span>
                    );
                  })}
                </div>
              )}

              <div className="usage-models">
                {modelsByTokens.map((m) => (
                  <ModelRow key={`${m.provider}:${m.model}`} m={m} maxVal={maxModel} mode="tokens" />
                ))}
              </div>
            </section>
          )}
        </>
      )}
    </>
  );
}

export function Usage({ client }: { client: DesktopClient }) {
  const [tab, setTab] = useState<Tab>("cost");
  const [range, setRange] = useState<UsageRange>("7d");
  const [limitStyle, setLimitStyle] = useState<LimitStyle>("bars");
  const [providers, setProviders] = useState<UsageProvider[] | null>(null);
  const [tokens, setTokens] = useState<UsageTokens | null | undefined>(undefined);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const ptr = useRef<HTMLDivElement>(null);
  useTick(30_000);

  const load = useCallback(
    async (r: UsageRange = range) => {
      setBusy(true);
      try {
        const res = await client.request({ t: "usage.get", range: r });
        if (res.t === "usage") {
          setProviders(res.providers);
          setTokens(res.tokens ?? null);
        } else if (res.t === "error") {
          throw new Error(res.error);
        }
        setErr(null);
      } catch (e) {
        setErr((e as Error).message);
      } finally {
        setBusy(false);
      }
    },
    [client, range],
  );

  useEffect(() => {
    void load(range);
    const t = setInterval(() => void load(range), 60_000);
    return () => clearInterval(t);
  }, [load, range]);

  usePullToRefresh(scroller, ptr, () => load(range));

  const limitAlerts = useMemo(() => {
    if (!providers) return 0;
    let n = 0;
    for (const p of providers) {
      const main = p.accounts.length > 1 && p.pooled ? p.pooled : p.accounts[0];
      for (const w of main.windows) {
        if (toneOf(w)) n++;
      }
    }
    return n;
  }, [providers]);

  const subtitle = useMemo(() => {
    if (busy && !providers) return "Reading usage…";
    if (busy) return "Refreshing…";
    if (tab === "cost" && tokens) {
      return `${fmtCost(tokens.cost)} est. spend · ${range}`;
    }
    if (tab === "tokens" && tokens) {
      const tot = tokens.input + tokens.output + tokens.cacheRead + tokens.cacheWrite;
      return `${fmtTokens(tot)} tokens · ${range}`;
    }
    if (!providers?.length) return "Cost, tokens & limits";
    let latest = 0;
    for (const p of providers) {
      const main = p.accounts.length > 1 && p.pooled ? p.pooled : p.accounts[0];
      if (main.at > latest) latest = main.at;
    }
    const status = limitAlerts ? `${limitAlerts} ${limitAlerts === 1 ? "limit" : "limits"} low` : "All plans healthy";
    return latest ? `${status} · ${freshness(latest).short}` : status;
  }, [busy, providers, tab, tokens, range, limitAlerts]);

  return (
    <div className="screen">
      <header className="bar">
        <button className="icon-btn" aria-label="Back" onClick={() => go("/settings")}>
          <Icon name="back" size={19} />
        </button>
        <div className="bar-title">
          <b>Usage &amp; limits</b>
          <span>{subtitle}</span>
        </div>
        <button
          className="icon-btn usage-refresh"
          data-busy={busy || undefined}
          aria-label="Refresh"
          onClick={() => {
            haptic(6);
            void load(range);
          }}
        >
          <Icon name="refresh" size={17} />
        </button>
      </header>
      <div className="ptr" ref={ptr} aria-hidden>
        <Icon name="refresh" size={15} />
      </div>
      <div className="scroll" ref={scroller}>
        <main className="usage">
          <div className="usage-tabs" data-active={tab} role="tablist" aria-label="Usage view">
            <span className="usage-tabs-slider" aria-hidden />
            <button
              type="button"
              role="tab"
              aria-selected={tab === "cost"}
              onClick={() => {
                haptic(5);
                setTab("cost");
              }}
            >
              Cost
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "tokens"}
              onClick={() => {
                haptic(5);
                setTab("tokens");
              }}
            >
              Tokens
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "limits"}
              onClick={() => {
                haptic(5);
                setTab("limits");
              }}
            >
              Limits
              {limitAlerts > 0 && <span className="usage-alert-dot" aria-hidden />}
            </button>
          </div>

          {err && <div className="inline-error">{err}</div>}

          {!providers && !err ? (
            <div className="usage-skeleton" aria-busy="true">
              <div className="usage-skel-card" />
              <div className="usage-skel-card" />
            </div>
          ) : tab === "cost" ? (
            <CostView tokens={tokens} range={range} onRange={setRange} />
          ) : tab === "tokens" ? (
            <TokensView tokens={tokens} range={range} onRange={setRange} />
          ) : (
            <>
              {providers && providers.length > 0 && (
                <div className="usage-range-bar">
                  <span>Plan limits</span>
                  <div className="usage-range-pills" role="group" aria-label="Limit display style">
                    {(["bars", "lines"] as const).map((s) => (
                      <button
                        key={s}
                        type="button"
                        aria-pressed={limitStyle === s}
                        onClick={() => {
                          haptic(5);
                          setLimitStyle(s);
                        }}
                      >
                        {s === "bars" ? "Bars" : "Lines"}
                      </button>
                    ))}
                  </div>
                </div>
              )}
              {providers?.map((p, idx) => (
                <ProviderCard key={p.id} p={p} index={idx} style={limitStyle} />
              ))}
              {providers && !providers.length && (
                <div className="usage-empty">
                  <Icon name="gauge" size={26} />
                  <b>No usage data yet</b>
                  <p>Open winfer on your desktop and refresh to sync your signed-in plans.</p>
                </div>
              )}
            </>
          )}
        </main>
      </div>
    </div>
  );
}
