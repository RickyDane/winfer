// Mockups of three directions for the chat's `visualize` reports. One sample report, four renderers
// (A, B, C and the current look for reference), shared SVG chart primitives.

const DATA = {
  title: "Build performance",
  subtitle: "winfer workspace · cargo build --release · Nov 12 – 23",
  stats: [
    { label: "Median incremental", value: "41s", delta: "-8%", good: true, note: "was 44.6s", spark: [52, 49, 50, 47, 46, 48, 44, 45, 43, 42, 41, 41] },
    { label: "Cache hit rate", value: "87%", delta: "+4 pts", good: true, note: "target 90%", spark: [78, 80, 79, 82, 83, 82, 85, 84, 86, 86, 87, 87] },
    { label: "Crates rebuilt", value: "14", delta: "+2", good: false, note: "of 38", spark: [11, 12, 10, 12, 13, 12, 12, 13, 14, 13, 14, 14] },
    { label: "Failed builds", value: "2", delta: "-3", good: true, note: "this week", spark: [3, 1, 2, 4, 2, 1, 1, 0, 1, 0, 1, 0] },
  ],
  line: {
    title: "Build time per day", unit: "s",
    labels: ["Nov 12", "Nov 13", "Nov 14", "Nov 15", "Nov 16", "Nov 17", "Nov 18", "Nov 19", "Nov 20", "Nov 21", "Nov 22", "Nov 23"],
    series: [
      { name: "Cold", values: [128, 124, 131, 122, 119, 112, 98, 95, 92, 88, 86, 84] },
      { name: "Incremental", values: [52, 49, 50, 47, 46, 48, 44, 45, 43, 42, 41, 41] },
    ],
    caption: "Cache rework merged Nov 17.",
  },
  hbar: {
    title: "Seconds per crate", unit: "s",
    labels: ["winfer-app", "gpui-kit", "winfer-backend", "relay", "viz_svg", "theme"],
    series: [{ name: "Build", values: [29, 18.2, 12.4, 6.4, 3.1, 1.2] }],
  },
  stack: {
    title: "Time by phase", unit: "s",
    labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    series: [
      { name: "Codegen", values: [18, 17, 19, 16, 15, 14, 13] },
      { name: "Linking", values: [12, 11, 12, 10, 9, 8, 8] },
      { name: "Type check", values: [8, 9, 7, 8, 6, 5, 5] },
    ],
  },
  pie: {
    title: "Code by language", unit: "",
    slices: [{ label: "Rust", value: 312000 }, { label: "TypeScript", value: 61000 }, { label: "CSS", value: 21000 }, { label: "Shell", value: 9000 }, { label: "Other", value: 9000, other: true }],
    center: { value: "412K", label: "lines" },
  },
  progress: {
    title: "Budgets",
    items: [
      { label: "CI minutes", value: 640, max: 1000, unit: " min" },
      { label: "Cache storage", value: 8.6, max: 10, unit: " GB" },
      { label: "Artifact retention", value: 12, max: 30, unit: " days" },
    ],
  },
  table: {
    title: "Slowest crates",
    columns: [{ label: "Crate" }, { label: "Time", align: "right" }, { label: "Δ vs last week", align: "right" }, { label: "Share", align: "right", bar: true }],
    rows: [["winfer-app", "29.0s", "+2.1s", "41%"], ["gpui-kit", "18.2s", "−0.8s", "26%"], ["winfer-backend", "12.4s", "−3.0s", "18%"], ["relay", "6.4s", "−0.2s", "9%"], ["viz_svg", "3.1s", "+0.4s", "4%"]],
  },
  text: "Numbers are wall-clock on the <code>m3-pro</code> runner; cold builds start from an empty <code>target/</code>.",
};

const NOTES = {
  a: "A · Instrument — one framed card per report: header with Chart/Data toggle + export, KPI strip with sparklines and delta chips, panels divided by hairlines instead of nested cards, gradient areas and crosshair tooltips.",
  b: "B · Editorial — no frames at all: the report reads like a figure in an article. Hero number, direct end-labels, lollipop bars highlighting the leader, a part-to-whole bar instead of a donut, ruled tables, tick meters.",
  c: "C · Bento — a tiled dashboard on a 12-column grid: every tile has its own Chart/Table tabs, legend chips toggle series (colors stay with the entity), data bars inside tables, status-aware meters.",
  now: "Current — today's renderer for reference: every block is its own raised card with a title and copy button.",
};

const state = { v: "b", p: "theme", w: "wide", theme: "dark" };
let uid = 0;
let specs;

// ---------- helpers ----------
const col = (i) => (i < 8 ? `var(--s${i})` : "var(--subtle)");
const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;");
function fmt(v, u = "") {
  const a = Math.abs(v);
  let s = a >= 1e6 ? (v / 1e6).toFixed(1).replace(/\.0$/, "") + "M" : a >= 1e4 ? (v / 1e3).toFixed(0) + "K" : Number.isInteger(v) ? v.toLocaleString() : v.toFixed(1);
  return s + (u || "");
}
function ticks(max, n = 4) {
  if (max <= 0) return [0, 1];
  const raw = max / n, p = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * p).find((s) => s >= raw);
  const out = [];
  for (let v = 0; v <= Math.ceil(max / step) * step + 1e-9; v += step) out.push(+v.toFixed(6));
  return out;
}
const ICON = {
  chart: `<svg viewBox="0 0 16 16"><path d="M2.5 13.5h11M4.5 11V7M8 11V4M11.5 11V8.5"/></svg>`,
  copy: `<svg viewBox="0 0 16 16"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M10.5 3.5v-.5a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h.5"/></svg>`,
  dl: `<svg viewBox="0 0 16 16"><path d="M8 2.5v8M4.5 7 8 10.5 11.5 7M3 13.5h10"/></svg>`,
  table: `<svg viewBox="0 0 16 16"><rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M2.5 6.5h11M6.5 6.5V13"/></svg>`,
  up: `<svg viewBox="0 0 10 10"><path d="M5 8V2M2.5 4.5 5 2l2.5 2.5"/></svg>`,
  down: `<svg viewBox="0 0 10 10"><path d="M5 2v6M2.5 5.5 5 8l2.5-2.5"/></svg>`,
  warn: `<svg viewBox="0 0 16 16"><path d="M8 2.5 14 13H2z"/><path d="M8 6.5v3M8 11.2v.1"/></svg>`,
};
const arrow = (d) => (String(d).trim().startsWith("-") || String(d).trim().startsWith("−") ? ICON.down : ICON.up);
const delta = (s, cls = "delta") => `<span class="${cls} ${s.good ? "good" : "bad"}">${arrow(s.delta)}${esc(s.delta.replace(/^[-+]/, ""))}</span>`;

// ---------- tooltip ----------
const tip = document.getElementById("tip");
function showTip(e, html) {
  tip.innerHTML = html;
  tip.classList.add("on");
  const r = tip.getBoundingClientRect();
  let x = e.clientX + 14, y = e.clientY + 14;
  if (x + r.width > innerWidth - 8) x = e.clientX - r.width - 14;
  if (y + r.height > innerHeight - 8) y = e.clientY - r.height - 14;
  tip.style.transform = `translate(${x}px, ${y}px)`;
}
const hideTip = () => tip.classList.remove("on");
const tipRow = (ci, name, val) => `<div class="tr"><i style="background:${col(ci)}"></i><span>${esc(name)}</span><b>${val}</b></div>`;

// ---------- chart primitives ----------
function spark(values, ci = 0, w = 76, h = 26) {
  const max = Math.max(...values), min = Math.min(...values), r = max - min || 1;
  const pts = values.map((v, i) => [(i / (values.length - 1)) * (w - 6) + 1, h - 4 - ((v - min) / r) * (h - 8)]);
  const d = pts.map((p, i) => (i ? "L" : "M") + p[0].toFixed(1) + "," + p[1].toFixed(1)).join("");
  const id = "sp" + uid++;
  const last = pts[pts.length - 1];
  return `<svg class="spark" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" preserveAspectRatio="none"><defs><linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" style="stop-color:${col(ci)};stop-opacity:.25"/><stop offset="1" style="stop-color:${col(ci)};stop-opacity:0"/></linearGradient></defs>
    <path d="${d}L${last[0]},${h}L1,${h}Z" fill="url(#${id})"/><path d="${d}" style="stroke:${col(ci)}" class="ln" stroke-width="1.5"/>
    <circle cx="${last[0]}" cy="${last[1]}" r="2.5" style="fill:${col(ci)}"/></svg>`;
}

function lineChart(el, spec, o = {}) {
  const W = el.clientWidth, H = o.height || 200;
  if (W < 160) return;
  const vis = spec.series.map((s, i) => ({ ...s, ci: i })).filter((s) => !s.hidden);
  const n = spec.labels.length;
  const t = ticks(Math.max(1, ...vis.flatMap((s) => s.values)), 4), top = t[t.length - 1];
  const L = 38, R = o.endLabels ? 104 : 12, T = 10, B = 26, pw = W - L - R, ph = H - T - B;
  const x = (i) => L + (i * pw) / (n - 1), y = (v) => T + ph - (v / top) * ph;
  const id = "lg" + uid++;
  let s = `<svg width="${W}" height="${H}" class="chart"><defs>${vis.map((v) => `<linearGradient id="${id}${v.ci}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" style="stop-color:${col(v.ci)};stop-opacity:${o.area === "gradient" ? 0.2 : 0.1}"/><stop offset="1" style="stop-color:${col(v.ci)};stop-opacity:${o.area === "gradient" ? 0 : 0.1}"/></linearGradient>`).join("")}</defs>`;
  for (const v of t) s += `<line x1="${L}" x2="${W - R}" y1="${y(v)}" y2="${y(v)}" class="${v === 0 ? "base" : "grid"}"/><text x="${L - 8}" y="${y(v)}" class="ax" text-anchor="end" dominant-baseline="middle">${fmt(v, spec.unit)}</text>`;
  const step = Math.ceil(n / Math.max(2, Math.floor(pw / 62)));
  spec.labels.forEach((l, i) => { if (i % step === 0) s += `<text x="${x(i)}" y="${H - 6}" class="ax" text-anchor="${i === 0 ? "start" : "middle"}">${esc(l)}</text>`; });
  if (o.marker != null) s += `<line x1="${x(o.marker)}" x2="${x(o.marker)}" y1="${T}" y2="${T + ph}" class="mark-line"/><text x="${x(o.marker) + 6}" y="${T + 10}" class="ax mark-txt">${esc(o.markerLabel || "")}</text>`;
  for (const v of vis) {
    const d = v.values.map((val, i) => (i ? "L" : "M") + x(i).toFixed(1) + "," + y(val).toFixed(1)).join("");
    if (o.area) s += `<path d="${d}L${x(n - 1)},${y(0)}L${x(0)},${y(0)}Z" fill="url(#${id}${v.ci})"/>`;
    s += `<path d="${d}" class="ln" style="stroke:${col(v.ci)}" stroke-width="2"/>`;
    const lv = v.values[n - 1];
    s += `<circle cx="${x(n - 1)}" cy="${y(lv)}" r="4" class="dot" style="fill:${col(v.ci)}"/>`;
  }
  if (o.endLabels) {
    const ls = vis.map((v) => ({ v, y: y(v.values[n - 1]) })).sort((a, b) => a.y - b.y);
    for (let i = 1; i < ls.length; i++) ls[i].y = Math.max(ls[i].y, ls[i - 1].y + 28);
    for (const l of ls) s += `<text x="${W - R + 12}" y="${l.y - 3}" class="end-v">${fmt(l.v.values[n - 1], spec.unit)}</text><text x="${W - R + 12}" y="${l.y + 11}" class="end-n">${esc(l.v.name)}</text>`;
  }
  s += `<g class="hov" style="display:none"><line class="cross" y1="${T}" y2="${T + ph}"/>${vis.map((v) => `<circle r="4" class="dot" style="fill:${col(v.ci)}"/>`).join("")}</g>`;
  s += `<rect class="hit" x="${L}" y="${T}" width="${pw}" height="${ph}" fill="transparent"/></svg>`;
  el.innerHTML = s;
  const g = el.querySelector(".hov"), hit = el.querySelector(".hit");
  hit.onmousemove = (e) => {
    const i = Math.max(0, Math.min(n - 1, Math.round(((e.offsetX - L) / pw) * (n - 1))));
    g.style.display = "";
    g.querySelector("line").setAttribute("x1", x(i)); g.querySelector("line").setAttribute("x2", x(i));
    g.querySelectorAll("circle").forEach((c, k) => { c.setAttribute("cx", x(i)); c.setAttribute("cy", y(vis[k].values[i])); });
    showTip(e, `<div class="th">${esc(spec.labels[i])}</div>` + vis.map((v) => tipRow(v.ci, v.name, fmt(v.values[i], spec.unit))).join(""));
  };
  hit.onmouseleave = () => { g.style.display = "none"; hideTip(); };
}

function columns(el, spec, o = {}) {
  const W = el.clientWidth, H = o.height || 200;
  if (W < 160) return;
  const vis = spec.series.map((s, i) => ({ ...s, ci: i })).filter((s) => !s.hidden);
  const n = spec.labels.length;
  const totals = spec.labels.map((_, i) => vis.reduce((a, v) => a + v.values[i], 0));
  const t = ticks(Math.max(1, ...totals), 4), top = t[t.length - 1];
  const L = 34, R = 8, T = o.capLabels ? 18 : 10, B = 26, pw = W - L - R, ph = H - T - B, band = pw / n;
  const bw = Math.min(o.barW || 24, band * 0.62);
  const y = (v) => T + ph - (v / top) * ph;
  let s = `<svg width="${W}" height="${H}" class="chart">`;
  for (const v of t) s += `<line x1="${L}" x2="${W - R}" y1="${y(v)}" y2="${y(v)}" class="${v === 0 ? "base" : "grid"}"/><text x="${L - 8}" y="${y(v)}" class="ax" text-anchor="end" dominant-baseline="middle">${fmt(v, spec.unit)}</text>`;
  spec.labels.forEach((l, i) => {
    const cx = L + band * i + band / 2, x0 = cx - bw / 2;
    s += `<rect class="band" data-i="${i}" x="${L + band * i}" y="${T}" width="${band}" height="${ph}"/>`;
    let acc = 0;
    vis.forEach((v, k) => {
      const h = (v.values[i] / top) * ph, yb = y(acc) - (k ? 2 : 0), yt = y(acc + v.values[i]), hh = Math.max(0, yb - yt);
      acc += v.values[i];
      const r = k === vis.length - 1 ? Math.min(4, hh) : 0;
      s += `<path pointer-events="none" style="fill:${col(v.ci)}" d="M${x0},${yb}V${yt + r}Q${x0},${yt} ${x0 + r},${yt}H${x0 + bw - r}Q${x0 + bw},${yt} ${x0 + bw},${yt + r}V${yb}Z"/>`;
    });
    if (o.capLabels) s += `<text x="${cx}" y="${y(totals[i]) - 6}" class="cap-v" text-anchor="middle">${fmt(totals[i], spec.unit)}</text>`;
    s += `<text x="${cx}" y="${H - 6}" class="ax" text-anchor="middle">${esc(l)}</text>`;
  });
  el.innerHTML = s + "</svg>";
  el.querySelectorAll(".band").forEach((b) => {
    const i = +b.dataset.i;
    b.onmousemove = (e) => showTip(e, `<div class="th">${esc(spec.labels[i])}<span>${fmt(totals[i], spec.unit)}</span></div>` + vis.slice().reverse().map((v) => tipRow(v.ci, v.name, fmt(v.values[i], spec.unit))).join(""));
    b.onmouseleave = hideTip;
  });
}

function hbars(el, spec, o = {}) {
  const v = spec.series[0].values, max = Math.max(...v), total = v.reduce((a, b) => a + b, 0);
  el.innerHTML = `<div class="hb ${o.lollipop ? "lolli" : ""}">` + spec.labels.map((l, i) => {
    const c = o.highlight == null || o.highlight === i ? col(o.color || 0) : "var(--mute-bar)";
    return `<div class="hb-row" data-i="${i}"><div class="hb-l">${esc(l)}</div><div class="hb-t"><div class="hb-b" style="width:${(v[i] / max) * 100}%;--c:${c}"></div></div><div class="hb-v">${fmt(v[i], spec.unit)}</div></div>`;
  }).join("") + "</div>";
  el.querySelectorAll(".hb-row").forEach((r) => {
    const i = +r.dataset.i;
    r.onmousemove = (e) => showTip(e, `<div class="th">${esc(spec.labels[i])}</div>` + tipRow(o.color || 0, spec.series[0].name, fmt(v[i], spec.unit)) + `<div class="tr sub"><span>Share of total</span><b>${Math.round((v[i] / total) * 100)}%</b></div>`);
    r.onmouseleave = hideTip;
  });
}

function arc(cx, cy, R, r, a0, a1) {
  const p = (rad, a) => [cx + rad * Math.sin(a), cy - rad * Math.cos(a)];
  const po = 1 / R, pi = 1 / r, large = a1 - a0 > Math.PI ? 1 : 0;
  const [x0, y0] = p(R, a0 + po), [x1, y1] = p(R, a1 - po), [x2, y2] = p(r, a1 - pi), [x3, y3] = p(r, a0 + pi);
  return `M${x0},${y0}A${R},${R} 0 ${large} 1 ${x1},${y1}L${x2},${y2}A${r},${r} 0 ${large} 0 ${x3},${y3}Z`;
}
const sliceColor = (sl, i) => (sl.other ? "var(--subtle)" : col(i));

function donut(el, spec, o = {}) {
  const S = o.size || 160, R = S / 2, r = R * (o.thin ? 0.78 : 0.64);
  const total = spec.slices.reduce((a, b) => a + b.value, 0);
  let a = 0, paths = "";
  spec.slices.forEach((sl, i) => {
    const a1 = a + (sl.value / total) * Math.PI * 2;
    paths += `<path class="sl" data-i="${i}" style="fill:${sliceColor(sl, i)}" d="${arc(R, R, R - 1, r, a, a1)}"/>`;
    a = a1;
  });
  const legend = spec.slices.map((sl, i) => `<div class="pl-row" data-i="${i}"><i style="background:${sliceColor(sl, i)}"></i><span class="pl-n">${esc(sl.label)}</span><span class="pl-v">${fmt(sl.value)}</span><span class="pl-p">${Math.round((sl.value / total) * 100)}%</span></div>`).join("");
  el.innerHTML = `<div class="pie ${o.stack ? "stack" : ""}"><div class="pie-c" style="width:${S}px;height:${S}px"><svg width="${S}" height="${S}">${paths}</svg>
    <div class="pie-h"><b>${esc(spec.center.value)}</b><span>${esc(spec.center.label)}</span></div></div><div class="pl">${legend}</div></div>`;
  const focus = (i) => el.querySelectorAll("[data-i]").forEach((n) => n.classList.toggle("dim", i != null && +n.dataset.i !== i));
  el.querySelectorAll("[data-i]").forEach((n) => {
    const i = +n.dataset.i, sl = spec.slices[i];
    n.onmousemove = (e) => { focus(i); showTip(e, tipRow(i, sl.label, `${fmt(sl.value)} · ${Math.round((sl.value / total) * 100)}%`).replace(col(i), sliceColor(sl, i))); };
    n.onmouseleave = () => { focus(null); hideTip(); };
  });
}

function partBar(el, spec) {
  const total = spec.slices.reduce((a, b) => a + b.value, 0);
  el.innerHTML = `<div class="pb"><div class="pb-bar">${spec.slices.map((sl, i) => `<div class="pb-s" data-i="${i}" style="flex:${sl.value};background:${sliceColor(sl, i)}"></div>`).join("")}</div>
    <div class="pb-lg">${spec.slices.map((sl, i) => `<div class="pb-i" data-i="${i}"><div class="pb-p"><i style="background:${sliceColor(sl, i)}"></i>${Math.round((sl.value / total) * 100)}%</div><div class="pb-n">${esc(sl.label)} · ${fmt(sl.value)}</div></div>`).join("")}</div></div>`;
  el.querySelectorAll("[data-i]").forEach((n) => {
    const i = +n.dataset.i, sl = spec.slices[i];
    n.onmousemove = (e) => { el.querySelectorAll("[data-i]").forEach((m) => m.classList.toggle("dim", +m.dataset.i !== i)); showTip(e, `<div class="th">${esc(sl.label)}</div><div class="tr sub"><span>Lines</span><b>${fmt(sl.value)}</b></div>`); };
    n.onmouseleave = () => { el.querySelectorAll(".dim").forEach((m) => m.classList.remove("dim")); hideTip(); };
  });
}

function meters(el, spec, o = {}) {
  el.innerHTML = `<div class="mt ${o.style || ""}">` + spec.items.map((m) => {
    const f = Math.min(1, m.value / m.max), hot = f >= 0.85 && o.status;
    let bar;
    if (o.style === "ticks") {
      const N = 30, on = Math.round(f * N);
      bar = `<div class="mt-ticks">${Array.from({ length: N }, (_, k) => `<i class="${k < on ? "on" : ""}"></i>`).join("")}</div>`;
    } else bar = `<div class="mt-track"><div class="mt-fill ${hot ? "hot" : ""}" style="width:${f * 100}%"></div></div>`;
    return `<div class="mt-row"><div class="mt-top"><span class="mt-l">${esc(m.label)}</span>${hot ? `<span class="mt-warn">${ICON.warn}Near limit</span>` : ""}<span class="mt-v"><b>${fmt(m.value)}</b> / ${fmt(m.max)}${m.unit}</span></div>${bar}</div>`;
  }).join("") + "</div>";
}

function table(el, spec, o = {}) {
  el.innerHTML = `<div class="tbl-wrap"><table class="tbl"><thead><tr>${spec.columns.map((c) => `<th class="${c.align || ""}">${esc(c.label)}</th>`).join("")}</tr></thead><tbody>${spec.rows.map((r) => `<tr>${r.map((v, j) => {
    const c = spec.columns[j] || {};
    if (c.bar && o.databar) return `<td class="right"><div class="dbar"><span>${esc(v)}</span><i style="width:${parseFloat(v)}%"></i></div></td>`;
    return `<td class="${c.align || ""} ${j ? "num" : ""}">${esc(v)}</td>`;
  }).join("")}</tr>`).join("")}</tbody></table></div>`;
}

function dataTable(kind, spec) {
  if (kind === "line" || kind === "stack") return { columns: [{ label: "" }, ...spec.series.map((s) => ({ label: s.name, align: "right" }))], rows: spec.labels.map((l, i) => [l, ...spec.series.map((s) => fmt(s.values[i], spec.unit))]) };
  if (kind === "hbar") return { columns: [{ label: "Crate" }, { label: "Time", align: "right" }], rows: spec.labels.map((l, i) => [l, fmt(spec.series[0].values[i], spec.unit)]) };
  if (kind === "pie" || kind === "part") {
    const t = spec.slices.reduce((a, b) => a + b.value, 0);
    return { columns: [{ label: "Language" }, { label: "Lines", align: "right" }, { label: "Share", align: "right" }], rows: spec.slices.map((s) => [s.label, fmt(s.value), Math.round((s.value / t) * 100) + "%"]) };
  }
  if (kind === "progress") return { columns: [{ label: "Budget" }, { label: "Used", align: "right" }, { label: "Limit", align: "right" }], rows: spec.items.map((m) => [m.label, fmt(m.value) + m.unit, fmt(m.max) + m.unit]) };
  return spec;
}

// ---------- per-direction options ----------
const OPTS = {
  a: { line: { area: "gradient", height: 210, marker: 5, markerLabel: "cache rework" }, stack: { height: 190 }, pie: { size: 148 }, hbar: {}, progress: {}, table: {} },
  b: { line: { endLabels: true, height: 230, marker: 5, markerLabel: "cache rework" }, stack: { height: 190, barW: 16, capLabels: true }, hbar: { lollipop: true, highlight: 0 }, part: {}, progress: { style: "ticks" }, table: {} },
  c: { line: { area: "wash", height: 196 }, stack: { height: 196, capLabels: true }, pie: { size: 136, thin: true, stack: true }, hbar: {}, progress: { status: true }, table: { databar: true } },
  now: { line: { area: "wash", height: 180 }, stack: { height: 180 }, pie: { size: 150 }, hbar: {}, progress: {}, table: {} },
};
const RENDER = { line: lineChart, stack: columns, hbar: hbars, pie: donut, part: partBar, progress: meters, table };

function mountAll(root) {
  root.querySelectorAll(".mount").forEach((el) => {
    const draw = () => {
      const kind = el.dataset.kind, spec = specs[el.dataset.key];
      if (el.dataset.view === "table" && kind !== "table") table(el, dataTable(kind, spec), OPTS[state.v].table);
      else RENDER[kind](el, spec, OPTS[state.v][kind] || {});
      el._w = el.clientWidth;
    };
    el._draw = draw;
    draw();
    new ResizeObserver(() => { if (el.clientWidth !== el._w) draw(); }).observe(el);
  });
}

const legend = (series, cls = "lg") => `<div class="${cls}">${series.map((s, i) => `<span><i style="background:${col(i)}"></i>${esc(s.name)}</span>`).join("")}</div>`;
const mount = (kind, key = kind) => `<div class="mount" data-kind="${kind}" data-key="${key}"></div>`;

// ---------- A · Instrument ----------
function renderA(d) {
  const panel = (kind, spec, cls = "", extra = "") => `<section class="pA ${cls}"><div class="pA-h"><span class="pA-t">${esc(spec.title)}</span>${extra}</div>${mount(kind)}${spec.caption ? `<div class="cap">${esc(spec.caption)}</div>` : ""}</section>`;
  return `<div class="rA">
    <header class="rA-h">
      <div class="rA-ic">${ICON.chart}</div>
      <div class="rA-tt"><div class="t">${esc(d.title)}</div><div class="st">${esc(d.subtitle)}</div></div>
      <div class="rA-act"><div class="seg" id="viewseg"><button class="on" data-view="chart">Chart</button><button data-view="table">Data</button></div>
        <button class="ib" title="Copy CSV">${ICON.copy}</button><button class="ib" title="Export PNG / SVG">${ICON.dl}</button></div>
    </header>
    <div class="rA-stats">${d.stats.map((s, i) => `<div class="sA"><div class="sA-l">${esc(s.label)}</div><div class="sA-v">${esc(s.value)}${delta(s, "chip")}</div><div class="sA-n">${esc(s.note)}</div>${spark(s.spark, i)}</div>`).join("")}</div>
    <div class="rA-grid">
      ${panel("line", d.line, "full", legend(d.line.series))}
      ${panel("hbar", d.hbar)}
      ${panel("pie", d.pie)}
      ${panel("stack", d.stack, "", legend(d.stack.series))}
      ${panel("progress", d.progress)}
      ${panel("table", d.table, "full")}
    </div>
    <footer class="rA-f">${d.text}</footer>
  </div>`;
}

// ---------- B · Editorial ----------
function renderB(d) {
  const [hero, ...rest] = d.stats;
  const sec = (kind, spec, extra = "", key = kind) => `<section class="sB"><h3>${esc(spec.title)}</h3>${extra}${mount(kind, key)}${spec.caption ? `<div class="cap">${esc(spec.caption)}</div>` : ""}</section>`;
  return `<article class="rB">
    <div class="rB-kick">${ICON.chart} Report</div>
    <h2>${esc(d.title)}</h2><p class="st">${esc(d.subtitle)}</p>
    <div class="rB-hero">
      <div class="hero"><div class="hv">${esc(hero.value)}</div><div class="hl">${esc(hero.label)} ${delta(hero)} <span class="muted">${esc(hero.note)}</span></div></div>
      <div class="rB-mini">${rest.map((s) => `<div><div class="mv">${esc(s.value)} ${delta(s)}</div><div class="ml">${esc(s.label)}</div></div>`).join("")}</div>
    </div>
    ${sec("line", d.line, legend(d.line.series, "lgB"))}
    <div class="rB-two">${sec("hbar", d.hbar)}${sec("part", d.pie, "", "pie")}</div>
    <div class="rB-two">${sec("stack", d.stack, legend(d.stack.series, "lgB sq"))}${sec("progress", d.progress)}</div>
    ${sec("table", d.table)}
    <p class="rB-note">${d.text}</p>
    <div class="rB-foot"><button>${ICON.copy}Copy data</button><button>${ICON.dl}PNG</button><button>${ICON.dl}SVG</button><button id="bdata">${ICON.table}View as tables</button></div>
  </article>`;
}

// ---------- C · Bento ----------
function renderC(d) {
  const tabs = (key) => `<div class="tabs" data-key="${key}"><button class="on" data-view="chart">Chart</button><button data-view="table">Table</button></div>`;
  const chips = (key, series) => `<div class="chips">${series.map((s, i) => `<button class="chip on" data-key="${key}" data-s="${i}"><i style="background:${col(i)}"></i>${esc(s.name)}</button>`).join("")}</div>`;
  const tile = (kind, spec, span, opts = {}) => `<section class="tC" style="--span:${span}"><div class="tC-h"><span class="t">${esc(spec.title)}</span>${opts.tabs === false ? "" : tabs(kind)}</div>${opts.chips || ""}${mount(kind)}</section>`;
  return `<div class="rC">
    <header class="rC-h"><div class="rC-ic">${ICON.chart}</div><div class="rC-tt"><div class="t">${esc(d.title)}</div><div class="st">${esc(d.subtitle)}</div></div>
      <div class="rC-act"><button class="ib">${ICON.copy}</button><button class="pill">${ICON.dl}Export</button></div></header>
    <div class="rC-grid">
      ${d.stats.map((s, i) => `<section class="tC stat" style="--span:3"><div class="sC-l">${esc(s.label)}</div><div class="sC-v">${esc(s.value)}</div><div class="sC-d">${delta(s)}<span>${esc(s.note)}</span></div><div class="sC-sp">${spark(s.spark, i, 160, 34)}</div></section>`).join("")}
      ${tile("line", d.line, 8, { chips: chips("line", d.line.series) })}
      ${tile("pie", d.pie, 4)}
      ${tile("stack", d.stack, 6, { chips: chips("stack", d.stack.series) })}
      ${tile("hbar", d.hbar, 6)}
      ${tile("progress", d.progress, 5, { tabs: false })}
      ${tile("table", d.table, 7, { tabs: false })}
    </div>
    <div class="rC-f">${d.text}</div>
  </div>`;
}

// ---------- Current (reference) ----------
function renderNow(d) {
  const card = (kind, spec, extra = "") => `<section class="cN"><div class="cN-h"><span>${esc(spec.title)}</span><button class="ib">${ICON.copy}</button></div>${extra}${mount(kind)}</section>`;
  return `<div class="rN"><div class="rN-h"><div><div class="t">${esc(d.title)}</div><div class="st">${esc(d.subtitle)}</div></div><span class="rN-x">${ICON.dl}PNG ${ICON.dl}SVG</span></div>
    <section class="cN"><div class="cN-h"><span>Summary</span><button class="ib">${ICON.copy}</button></div><div class="nStats">${d.stats.map((s) => `<div><div class="l">${esc(s.label)}</div><div class="v">${esc(s.value)}</div><div class="dd">${esc(s.delta)}</div></div>`).join("")}</div></section>
    ${card("line", d.line, legend(d.line.series))}
    <div class="rN-two">${card("hbar", d.hbar)}${card("pie", d.pie)}</div>
    ${card("stack", d.stack, legend(d.stack.series))}
    <div class="rN-two">${card("progress", d.progress)}${card("table", d.table)}</div>
    <div class="rN-t">${d.text}</div></div>`;
}

// ---------- wiring ----------
const report = document.getElementById("report");
function render() {
  specs = JSON.parse(JSON.stringify(DATA));
  document.documentElement.dataset.theme = state.theme;
  document.documentElement.dataset.palette = state.p;
  document.getElementById("chat").dataset.w = state.w;
  report.dataset.v = state.v;
  report.innerHTML = { a: renderA, b: renderB, c: renderC, now: renderNow }[state.v](specs);
  document.getElementById("notes").textContent = NOTES[state.v];
  mountAll(report);
}

report.addEventListener("click", (e) => {
  const seg = e.target.closest("#viewseg button");
  if (seg) {
    seg.parentElement.querySelectorAll("button").forEach((b) => b.classList.toggle("on", b === seg));
    report.querySelectorAll(".mount").forEach((m) => { m.dataset.view = seg.dataset.view; m._draw(); });
    return;
  }
  if (e.target.closest("#bdata")) {
    const b = e.target.closest("#bdata"), on = b.classList.toggle("on");
    report.querySelectorAll(".mount").forEach((m) => { m.dataset.view = on ? "table" : "chart"; m._draw(); });
    return;
  }
  const tab = e.target.closest(".tabs button");
  if (tab) {
    tab.parentElement.querySelectorAll("button").forEach((b) => b.classList.toggle("on", b === tab));
    const tileEl = tab.closest(".tC"), m = tileEl.querySelector(".mount");
    m.dataset.view = tab.dataset.view;
    tileEl.classList.toggle("as-table", tab.dataset.view === "table");
    m._draw();
    return;
  }
  const chip = e.target.closest(".chip");
  if (chip) {
    const s = specs[chip.dataset.key].series[+chip.dataset.s];
    const visible = specs[chip.dataset.key].series.filter((x) => !x.hidden).length;
    if (!s.hidden && visible === 1) return;
    s.hidden = !s.hidden;
    chip.classList.toggle("on", !s.hidden);
    chip.closest(".tC").querySelector(".mount")._draw();
  }
});

const group = (id, key) => document.getElementById(id).addEventListener("click", (e) => {
  const b = e.target.closest("button");
  if (!b) return;
  state[key] = b.dataset[key];
  document.querySelectorAll(`#${id} button`).forEach((x) => x.classList.toggle("on", x === b));
  render();
});
group("variants", "v");
group("widths", "w");
group("palettes", "p");
document.getElementById("theme").onchange = (e) => { state.theme = e.target.value; render(); };
render();
