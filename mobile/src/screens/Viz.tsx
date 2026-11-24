import type { ReactNode } from "react";
import type { Report, ReportBlock, Series } from "../protocol";
import { Markdown } from "./Markdown";
import { arcs, color, fmt, ticks } from "./vizMath";

type Block<T extends ReportBlock["type"]> = Extract<ReportBlock, { type: T }>;

/** A `visualize` call as the desktop's chat draws it: title, then its blocks stacked in one column. */
export function VizReport({ report }: { report: Report }) {
  return (
    <section className="viz">
      {(report.title || report.subtitle) && (
        <header className="viz-head">
          {report.title && <h3>{report.title}</h3>}
          {report.subtitle && <p>{report.subtitle}</p>}
        </header>
      )}
      {report.blocks.map((b, i) => (
        <figure key={i} className="viz-block" data-kind={b.type}>
          {b.title && <figcaption className="viz-title">{b.title}</figcaption>}
          <BlockBody b={b} />
          {b.caption && <p className="viz-caption">{b.caption}</p>}
        </figure>
      ))}
    </section>
  );
}

function BlockBody({ b }: { b: ReportBlock }): ReactNode {
  switch (b.type) {
    case "stats":
      return <Stats b={b} />;
    case "bar":
      return b.horizontal ? <HBars b={b} /> : <Columns b={b} />;
    case "line":
      return <Lines b={b} />;
    case "pie":
      return <Pie b={b} />;
    case "table":
      return <Table b={b} />;
    case "progress":
      return <Progress b={b} />;
    case "text":
      return <Markdown text={b.markdown} />;
  }
}

function Legend({ names }: { names: string[] }) {
  if (names.length < 2) return null;
  return (
    <ul className="viz-legend">
      {names.map((n, i) => (
        <li key={i}>
          <i style={{ background: color(i) }} />
          {n}
        </li>
      ))}
    </ul>
  );
}

const names = (series: Series[]) => series.map((s) => s.name);

function Stats({ b }: { b: Block<"stats"> }) {
  return (
    <div className="viz-stats">
      {b.items.map((it, i) => {
        const d = it.delta?.trim();
        const tone = d?.startsWith("-") || d?.startsWith("−") ? "down" : d?.startsWith("+") ? "up" : "flat";
        return (
          <div key={i} className="viz-stat">
            <span className="viz-stat-label">{it.label}</span>
            <b className="viz-stat-value">{it.value}</b>
            {d && <span className="viz-stat-delta" data-tone={tone}>{d}</span>}
            {it.note && <span className="viz-stat-note">{it.note}</span>}
          </div>
        );
      })}
    </div>
  );
}

function HBars({ b }: { b: Block<"bar"> }) {
  const totals = b.labels.map((_, i) => (b.stacked ? b.series.reduce((a, s) => a + Math.max(0, s.values[i] ?? 0), 0) : Math.max(0, ...b.series.map((s) => s.values[i] ?? 0))));
  const max = Math.max(...totals, 1e-9);
  return (
    <>
      <Legend names={names(b.series)} />
      <div className="viz-hbars">
        {b.labels.map((label, i) => (
          <div key={i} className="viz-hbar">
            <span className="viz-hbar-label">{label}</span>
            <div className="viz-hbar-tracks">
              {b.stacked ? (
                <div className="viz-track">
                  {b.series.map((s, si) => (
                    <span key={si} className="viz-seg" style={{ width: `${(Math.max(0, s.values[i] ?? 0) / max) * 100}%`, background: color(si) }} />
                  ))}
                  <em>{fmt(totals[i] ?? 0, b.unit)}</em>
                </div>
              ) : (
                b.series.map((s, si) => (
                  <div key={si} className="viz-track">
                    <span className="viz-seg" style={{ width: `${(Math.max(0, s.values[i] ?? 0) / max) * 100}%`, background: color(si) }} />
                    <em>{fmt(s.values[i] ?? 0, b.unit)}</em>
                  </div>
                ))
              )}
            </div>
          </div>
        ))}
      </div>
    </>
  );
}

// Shared plot box for vertical bars and lines (SVG scales with the column's width).
const W = 320;
const H = 190;
const PAD = { l: 38, r: 8, t: 8, b: 24 };

function Axes({ labels, ys, y }: { labels: string[]; ys: number[]; y: (v: number) => number }) {
  const step = Math.max(1, Math.ceil(labels.length / 6));
  const x = (i: number) => PAD.l + ((i + 0.5) / labels.length) * (W - PAD.l - PAD.r);
  return (
    <g className="viz-axes">
      {ys.map((v) => (
        <g key={v}>
          <line x1={PAD.l} x2={W - PAD.r} y1={y(v)} y2={y(v)} />
          <text x={PAD.l - 5} y={y(v) + 3} textAnchor="end">
            {fmt(v)}
          </text>
        </g>
      ))}
      {labels.map((l, i) =>
        i % step === 0 ? (
          <text key={i} x={x(i)} y={H - 7} textAnchor="middle">
            {l.length > 8 ? `${l.slice(0, 7)}…` : l}
          </text>
        ) : null,
      )}
    </g>
  );
}

function range(series: Series[], stacked = false): [number, number] {
  const n = series[0]?.values.length ?? 0;
  const all: number[] = [];
  for (let i = 0; i < n; i++) {
    if (stacked) all.push(series.reduce((a, s) => a + Math.max(0, s.values[i] ?? 0), 0));
    else all.push(...series.map((s) => s.values[i] ?? 0));
  }
  return [Math.min(0, ...all), Math.max(0, ...all)];
}

function Columns({ b }: { b: Block<"bar"> }) {
  const [lo, hi] = range(b.series, b.stacked);
  const ys = ticks(lo, hi);
  const top = ys[ys.length - 1] ?? 1;
  const bottom = ys[0] ?? 0;
  const y = (v: number) => PAD.t + (1 - (v - bottom) / (top - bottom || 1)) * (H - PAD.t - PAD.b);
  const slot = (W - PAD.l - PAD.r) / b.labels.length;
  const group = slot * 0.7;
  return (
    <>
      <Legend names={names(b.series)} />
      <svg className="viz-svg" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={b.title ?? "Bar chart"}>
        <Axes labels={b.labels} ys={ys} y={y} />
        {b.labels.map((_, i) => {
          const left = PAD.l + slot * i + (slot - group) / 2;
          let acc = 0;
          return b.series.map((s, si) => {
            const v = s.values[i] ?? 0;
            const w = b.stacked ? group : group / b.series.length;
            const x = b.stacked ? left : left + w * si;
            const [from, to] = b.stacked ? [acc, (acc += Math.max(0, v))] : [0, v];
            const y1 = y(Math.max(from, to));
            const h = Math.max(0, Math.abs(y(from) - y(to)));
            return <rect key={`${i}-${si}`} x={x} y={y1} width={Math.max(1, w - 1)} height={h} rx={2} fill={color(si)} />;
          });
        })}
      </svg>
    </>
  );
}

function Lines({ b }: { b: Block<"line"> }) {
  const [lo, hi] = range(b.series);
  const ys = ticks(lo, hi);
  const top = ys[ys.length - 1] ?? 1;
  const bottom = ys[0] ?? 0;
  const y = (v: number) => PAD.t + (1 - (v - bottom) / (top - bottom || 1)) * (H - PAD.t - PAD.b);
  const n = b.labels.length;
  const x = (i: number) => PAD.l + ((i + 0.5) / n) * (W - PAD.l - PAD.r);
  return (
    <>
      <Legend names={names(b.series)} />
      <svg className="viz-svg" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={b.title ?? "Line chart"}>
        <Axes labels={b.labels} ys={ys} y={y} />
        {b.series.map((s, si) => {
          const pts = s.values.map((v, i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`);
          return (
            <g key={si} stroke={color(si)} fill={color(si)}>
              {b.area && n > 1 && <polygon points={`${x(0).toFixed(1)},${y(0).toFixed(1)} ${pts.join(" ")} ${x(n - 1).toFixed(1)},${y(0).toFixed(1)}`} stroke="none" opacity={0.18} />}
              <polyline points={pts.join(" ")} fill="none" strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
              {n <= 24 && s.values.map((v, i) => <circle key={i} cx={x(i)} cy={y(v)} r={2.4} stroke="none" />)}
            </g>
          );
        })}
      </svg>
    </>
  );
}

function Pie({ b }: { b: Block<"pie"> }) {
  const total = b.slices.reduce((a, s) => a + s.value, 0) || 1;
  const R = 56; // outer radius
  const width = b.ring ? 22 : R; // a pie is a ring as wide as its radius
  const rad = R - width / 2;
  const circ = 2 * Math.PI * rad;
  const spans = arcs(b.slices.map((s) => s.value));
  return (
    <div className="viz-pie">
      <svg viewBox="0 0 140 140" role="img" aria-label={b.title ?? "Pie chart"}>
        <g transform="rotate(-90 70 70)" fill="none">
          {b.slices.map((_, i) => {
            const [from, to] = spans[i] as [number, number];
            if (to - from <= 0) return null;
            return <circle key={i} cx={70} cy={70} r={rad} stroke={color(i)} strokeWidth={width} strokeDasharray={`${(to - from) * circ} ${circ}`} strokeDashoffset={-from * circ} />;
          })}
        </g>
        {b.center && (
          <g textAnchor="middle" className="viz-pie-center">
            <text x={70} y={b.center.label ? 70 : 76} className="v">
              {b.center.value}
            </text>
            {b.center.label && (
              <text x={70} y={86} className="l">
                {b.center.label}
              </text>
            )}
          </g>
        )}
      </svg>
      <ul className="viz-legend col">
        {b.slices.map((s, i) => (
          <li key={i}>
            <i style={{ background: color(i) }} />
            <span className="viz-legend-name">{s.label}</span>
            <b>{fmt(s.value, b.unit)}</b>
            <span className="viz-legend-pct">{Math.round((s.value / total) * 100)}%</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Table({ b }: { b: Block<"table"> }) {
  return (
    <div className="viz-table-wrap">
      <table className="viz-table">
        <thead>
          <tr>
            {b.columns.map((c, i) => (
              <th key={i} style={{ textAlign: c.align }}>
                {c.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {b.rows.map((r, ri) => (
            <tr key={ri}>
              {r.map((cell, ci) => (
                <td key={ci} style={{ textAlign: b.columns[ci]?.align }}>
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Progress({ b }: { b: Block<"progress"> }) {
  return (
    <div className="viz-progress">
      {b.items.map((m, i) => {
        const pct = Math.max(0, Math.min(1, m.value / m.max));
        return (
          <div key={i} className="viz-meter">
            <div className="viz-meter-head">
              <span>{m.label}</span>
              <b>{m.max === 100 ? `${fmt(m.value)}%` : `${fmt(m.value)} / ${fmt(m.max)}`}</b>
            </div>
            <div className="viz-meter-track">
              <span style={{ width: `${pct * 100}%`, background: pct >= 1 ? "var(--ok)" : "var(--accent)" }} />
            </div>
          </div>
        );
      })}
    </div>
  );
}
