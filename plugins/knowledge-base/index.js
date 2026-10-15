// Knowledge Base: a graphify (https://pypi.org/project/graphifyy/) knowledge graph of each
// workspace's project. The page shows it as a live 3D galaxy (WebGL2, a force layout in a worker)
// where you search, trace paths, see what a change would affect and watch agents work; agents
// query the same graph through agent tools (kb_query, kb_explain, kb_path, kb_affected, …).
//
// graphify runs through `api.vault.exec` (a shell in the project folder, no secrets unless a
// headless build needs an API key). The graph lives in `<project>/graphify-out/`.

const STORAGE = "winfer.plugin.knowledge-base";
const OUT = "graphify-out";
const API_KEY = "ANTHROPIC_API_KEY";
const INSTALL = "uv tool install graphifyy";
/** What agent tools return at most, so a big answer doesn't flood an agent's context. */
const TOOL_OUTPUT_MAX = 12_000;
/** Relations a change travels back along: who calls, imports or extends a node is affected by it. */
const IMPACT_RELATIONS = new Set(["calls", "references", "imports", "imports_from", "re_exports", "inherits", "extends", "implements", "uses", "mixes_in", "embeds"]);
/** Relations that get particles flowing from source to target. */
const FLOW_RELATIONS = new Set(["calls", "imports", "imports_from", "references", "uses", "inherits", "extends", "implements", "re_exports"]);
const CONFIDENCE = ["EXTRACTED", "INFERRED", "AMBIGUOUS"];
const TYPE_LABELS = { code: "Code", document: "Docs", concept: "Concepts", rationale: "Rationale", paper: "Papers", image: "Images", video: "Media" };
const IS_WIN = /Win/i.test(navigator.platform ?? "") || /Windows/i.test(navigator.userAgent);
const REDUCED_MOTION = globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
const SNAPSHOT = /^\d{4}-\d{2}-\d{2}(?:_(\d+))?$/;

// Helpers ----------------------------------------------------------------------------------------

/** Builds an element: h("div", { class: "x", onclick }, child, "text", …). */
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else if (key in el && typeof value !== "string") el[key] = value;
    else el.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

const ICONS = {
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
  refresh: '<path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><path d="M21 3v5h-5"/>',
  sparkles: '<path d="M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z"/><path d="M19 15.5l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7z"/>',
  bot: '<rect x="4" y="8" width="16" height="12" rx="3"/><path d="M12 4v4"/><circle cx="9" cy="14" r="1"/><circle cx="15" cy="14" r="1"/>',
  zap: '<path d="M13 2 4 14h7l-1 8 9-12h-7z"/>',
  route: '<circle cx="6" cy="19" r="2.5"/><circle cx="18" cy="5" r="2.5"/><path d="M8.5 19H16a3.5 3.5 0 0 0 0-7H8a3.5 3.5 0 0 1 0-7h7.5"/>',
  orbit: '<circle cx="12" cy="12" r="2.2"/><ellipse cx="12" cy="12" rx="10" ry="4.2"/><ellipse cx="12" cy="12" rx="10" ry="4.2" transform="rotate(60 12 12)"/><ellipse cx="12" cy="12" rx="10" ry="4.2" transform="rotate(120 12 12)"/>',
  play: '<path d="M7 4.5v15l12.5-7.5z"/>',
  pause: '<path d="M8 5v14M16 5v14"/>',
  tag: '<path d="M3 12V4h8l10 10-8 8z"/><circle cx="7.5" cy="7.5" r="1.2"/>',
  sliders: '<path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h11M19 18h1"/><circle cx="15" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="17" cy="18" r="2"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>',
  external: '<path d="M14 4h6v6"/><path d="M20 4 11 13"/><path d="M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5"/>',
  copy: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/>',
  alert: '<path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/><path d="M12 9v4M12 17h.01"/>',
  check: '<path d="m5 12 5 5 9-10"/>',
  file: '<path d="M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8z"/><path d="M14 3v5h5"/>',
  git: '<circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="8" r="2.5"/><path d="M6 8.5v7M18 10.5c0 4-6 3-10 6"/>',
  crosshair: '<circle cx="12" cy="12" r="8"/><path d="M12 2v4M12 18v4M2 12h4M18 12h4"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  key: '<circle cx="8" cy="15" r="4"/><path d="m11 12 9-9M17 6l3 3"/>',
  book: '<path d="M4 5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2z"/><path d="M4 19V5"/>',
  layers: '<path d="m12 3 9 5-9 5-9-5z"/><path d="m3 13 9 5 9-5"/>',
  send: '<path d="M4 12 20 4l-6 16-3-7z"/>',
  terminal: '<path d="m5 8 4 4-4 4M12 16h7"/>',
  eye: '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>',
  chevron: '<path d="m6 9 6 6 6-6"/>',
  arrowRight: '<path d="M5 12h14M13 6l6 6-6 6"/>',
  arrowLeft: '<path d="M19 12H5M11 6l-6 6 6 6"/>',
  target: '<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="5"/><circle cx="12" cy="12" r="1"/>',
  grid: '<rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/>',
  wand: '<path d="m4 20 11-11M14 4v3M19 9h-3M17.5 5.5l-2 2M11 4l.5 1.5M20 13l-1.5-.5"/>',
};

/** A stroked 24×24 icon from ICONS. */
function icon(name, cls = "kb-i") {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", cls);
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.8");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.innerHTML = ICONS[name] ?? "";
  return svg;
}

/** Quotes one shell argument; on Windows (cmd) characters it would interpret are dropped. */
function q(value) {
  const s = String(value ?? "").replace(/\0/g, "");
  if (IS_WIN) return `"${s.replace(/["%^&|<>!\r\n]/g, " ")}"`;
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

const pathJoin = (...parts) => parts.join(IS_WIN ? "\\" : "/");
const clamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const plural = (n, word) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
const baseName = (p) => String(p).split(/[\\/]/).filter(Boolean).pop() ?? String(p);
const truncate = (text, max) => (text.length > max ? `${text.slice(0, max)}\n… (cut at ${max.toLocaleString()} characters)` : text);

function ago(ms) {
  if (!ms) return "never";
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86_400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86_400)} d ago`;
}

function elapsed(ms) {
  const s = Math.floor(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}

/** A seeded random generator, so layouts come out the same each time. */
function mulberry32(seed) {
  return () => {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function hashString(s) {
  let x = 2166136261;
  for (let i = 0; i < s.length; i++) x = Math.imul(x ^ s.charCodeAt(i), 16777619);
  return x >>> 0;
}

// Colors -----------------------------------------------------------------------------------------

/** OKLCH to sRGB (0–1), so every community color looks equally bright. */
function oklch(L, C, hueDeg) {
  const hr = (hueDeg * Math.PI) / 180;
  const a = C * Math.cos(hr);
  const b = C * Math.sin(hr);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const enc = (x) => {
    x = clamp(x, 0, 1);
    return x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055;
  };
  return [
    enc(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    enc(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    enc(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
  ];
}

/** Community color by size rank: golden-angle hues, tuned for the dark or light background. */
const communityColor = (rank, light) => oklch(light ? 0.58 : 0.78, light ? 0.17 : 0.15, (rank * 137.508 + 262) % 360);
const css = ([r, g, b], alpha = 1) => `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)} / ${alpha})`;

/** Parses a CSS color (hex or rgb()) from a custom property into 0–1 RGB. */
function cssColor(name, fallback) {
  const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const hex = /^#([0-9a-f]{6})/i.exec(raw);
  if (hex) return [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16) / 255);
  const rgb = /rgba?\((\d+)[ ,]+(\d+)[ ,]+(\d+)/.exec(raw);
  if (rgb) return [rgb[1], rgb[2], rgb[3]].map((x) => Number(x) / 255);
  return fallback;
}

const providerColor = (provider) =>
  cssColor(`--color-series-${provider}`, null) ?? (provider === "claude" ? cssColor("--color-claude", [0.85, 0.47, 0.34]) : cssColor("--color-accent", [0.43, 0.49, 1]));

// Parsing ----------------------------------------------------------------------------------------

/**
 * graph.json (networkx node-link data from graphify) in a compact form: parallel arrays for
 * nodes, typed arrays for edges, and adjacency in CSR form for fast neighbor lookups.
 */
function compactGraph(raw, names = new Map()) {
  const rawNodes = Array.isArray(raw?.nodes) ? raw.nodes : [];
  const rawLinks = Array.isArray(raw?.links) ? raw.links : Array.isArray(raw?.edges) ? raw.edges : [];
  const n = rawNodes.length;
  const index = new Map();
  const ids = new Array(n);
  const labels = new Array(n);
  const types = new Array(n);
  const files = new Array(n);
  const locs = new Array(n);
  const comm = new Int32Array(n);
  rawNodes.forEach((node, i) => {
    const id = String(node.id ?? i);
    index.set(id, i);
    ids[i] = id;
    labels[i] = String(node.label ?? node.norm_label ?? id);
    types[i] = String(node.file_type ?? "other");
    files[i] = node.source_file ? String(node.source_file) : "";
    locs[i] = node.source_location ? String(node.source_location) : "";
    comm[i] = Number.isFinite(Number(node.community)) && node.community !== null ? Number(node.community) : -1;
  });

  const relations = [];
  const relIndex = new Map();
  const src = [];
  const dst = [];
  const rel = [];
  const conf = [];
  const weight = [];
  for (const link of rawLinks) {
    const a = index.get(String(link.source?.id ?? link.source));
    const b = index.get(String(link.target?.id ?? link.target));
    if (a === undefined || b === undefined || a === b) continue;
    const name = String(link.relation ?? "related");
    if (!relIndex.has(name)) {
      relIndex.set(name, relations.length);
      relations.push(name);
    }
    src.push(a);
    dst.push(b);
    rel.push(relIndex.get(name));
    conf.push(Math.max(0, CONFIDENCE.indexOf(String(link.confidence ?? "EXTRACTED"))));
    weight.push(Number(link.weight) > 0 ? Number(link.weight) : 1);
  }
  const m = src.length;
  const degree = new Int32Array(n);
  for (let e = 0; e < m; e++) {
    degree[src[e]]++;
    degree[dst[e]]++;
  }
  const adjStart = new Int32Array(n + 1);
  for (let i = 0; i < n; i++) adjStart[i + 1] = adjStart[i] + degree[i];
  const fill = adjStart.slice(0, n);
  const adjEdge = new Int32Array(m * 2);
  for (let e = 0; e < m; e++) {
    adjEdge[fill[src[e]]++] = e;
    adjEdge[fill[dst[e]]++] = e;
  }

  // Communities by size; the rank picks the color and the spot in the galaxy.
  const sizes = new Map();
  for (let i = 0; i < n; i++) sizes.set(comm[i], (sizes.get(comm[i]) ?? 0) + 1);
  const communities = [...sizes.entries()]
    .sort((x, y) => y[1] - x[1] || x[0] - y[0])
    .map(([id, count], rank) => ({ id, rank, count, name: names.get(id) ?? (id < 0 ? "Unclustered" : `Community ${id}`) }));
  const communityById = new Map(communities.map((c) => [c.id, c]));
  const commRank = new Int32Array(n);
  for (let i = 0; i < n; i++) commRank[i] = communityById.get(comm[i]).rank;

  const typeCounts = new Map();
  for (const t of types) typeCounts.set(t, (typeCounts.get(t) ?? 0) + 1);

  const byLabel = new Map();
  labels.forEach((label, i) => {
    const list = byLabel.get(label);
    if (list) list.push(i);
    else byLabel.set(label, [i]);
  });
  // Per source file, the file's own node (else its best-connected one): where agents "are".
  const byFile = new Map();
  const fileScore = (i) => (labels[i] === baseName(files[i]) ? 1e9 : 0) + degree[i];
  files.forEach((file, i) => {
    if (!file) return;
    const best = byFile.get(file);
    if (best === undefined || fileScore(i) > fileScore(best)) byFile.set(file, i);
  });

  const hyperedges = (Array.isArray(raw?.hyperedges) ? raw.hyperedges : [])
    .map((x) => ({
      label: String(x.label ?? x.id ?? "Group"),
      relation: String(x.relation ?? ""),
      confidence: String(x.confidence ?? ""),
      nodes: (Array.isArray(x.nodes) ? x.nodes : []).map((id) => index.get(String(id))).filter((i) => i !== undefined),
    }))
    .filter((x) => x.nodes.length > 1);

  return {
    n,
    m,
    ids,
    labels,
    types,
    files,
    locs,
    comm,
    commRank,
    degree,
    src: Int32Array.from(src),
    dst: Int32Array.from(dst),
    rel: Uint16Array.from(rel),
    conf: Uint8Array.from(conf),
    weight: Float32Array.from(weight),
    relations,
    adjStart,
    adjEdge,
    communities,
    communityById,
    typeCounts,
    byLabel,
    byFile,
    hyperedges,
    index,
    commit: raw?.built_at_commit ? String(raw.built_at_commit) : null,
    directed: !!raw?.directed,
  };
}

/** The other end of edge `e` seen from node `i`. */
const otherEnd = (g, e, i) => (g.src[e] === i ? g.dst[e] : g.src[e]);

function neighbors(g, i) {
  const out = new Set();
  for (let k = g.adjStart[i]; k < g.adjStart[i + 1]; k++) out.add(otherEnd(g, g.adjEdge[k], i));
  return out;
}

/** Shortest path from a to b ignoring direction (BFS); [] if they aren't connected. */
function shortestPath(g, a, b) {
  if (a === b) return [a];
  const prev = new Int32Array(g.n).fill(-1);
  prev[a] = a;
  const queue = [a];
  for (let head = 0; head < queue.length; head++) {
    const i = queue[head];
    for (let k = g.adjStart[i]; k < g.adjStart[i + 1]; k++) {
      const j = otherEnd(g, g.adjEdge[k], i);
      if (prev[j] !== -1) continue;
      prev[j] = i;
      if (j === b) {
        const path = [b];
        for (let x = b; x !== a; x = prev[x]) path.push(prev[x]);
        return path.reverse();
      }
      queue.push(j);
    }
  }
  return [];
}

/** Nodes that depend on `origin` (call, import, extend… it), by depth: Map<node, depth>. */
function affected(g, origin, maxDepth = 2) {
  const depth = new Map([[origin, 0]]);
  let frontier = [origin];
  for (let d = 1; d <= maxDepth && frontier.length; d++) {
    const next = [];
    for (const i of frontier) {
      for (let k = g.adjStart[i]; k < g.adjStart[i + 1]; k++) {
        const e = g.adjEdge[k];
        if (g.dst[e] !== i || !IMPACT_RELATIONS.has(g.relations[g.rel[e]])) continue;
        const j = g.src[e];
        if (depth.has(j)) continue;
        depth.set(j, d);
        next.push(j);
      }
    }
    frontier = next;
  }
  return depth;
}

/** The edge between a and b (either direction), or -1. */
function edgeBetween(g, a, b) {
  for (let k = g.adjStart[a]; k < g.adjStart[a + 1]; k++) {
    const e = g.adjEdge[k];
    if (otherEnd(g, e, a) === b) return e;
  }
  return -1;
}

/** Finds a node by label (and file, if known); the best connected wins ties. */
function findNode(g, label, file) {
  const list = g.byLabel.get(label);
  if (!list) return -1;
  const pick = file ? list.filter((i) => g.files[i] === file || g.files[i].endsWith(file) || file.endsWith(g.files[i])) : list;
  return (pick.length ? pick : list).reduce((best, i) => (best < 0 || g.degree[i] > g.degree[best] ? i : best), -1);
}

/**
 * Nodes named in graphify's CLI output (query, explain, path, affected), in order of appearance,
 * so the galaxy can light up what an agent was just told.
 */
function nodesInOutput(g, text) {
  const found = [];
  const seen = new Set();
  const add = (label, file) => {
    const i = findNode(g, label.trim(), file?.trim());
    if (i >= 0 && !seen.has(i)) {
      seen.add(i);
      found.push(i);
    }
  };
  for (const line of String(text).split("\n")) {
    let match;
    if ((match = /^NODE (.+?) \[src=(\S*)/.exec(line))) add(match[1], match[2]);
    else if ((match = /^Node: (.+)$/.exec(line))) add(match[1]);
    else if ((match = /^\s*(?:<--|-->) (.+?) \[[^\]]+\] \[[A-Z]+\]/.exec(line))) add(match[1]);
    else if ((match = /^- (.+?) \[[^\]]+\] (\S+?):L\d+/.exec(line))) add(match[1], match[2]);
    else if (/ (?:<--\S+ \[[A-Z]+\]--|--\S+ \[[A-Z]+\]-->) /.test(line)) {
      for (const part of line.trim().split(/ (?:<--\S+ \[[A-Z]+\]--|--\S+ \[[A-Z]+\]-->) /)) add(part);
    }
  }
  return found;
}

/** GRAPH_REPORT.md: summary lines, community names, god nodes, surprises and groups. */
function parseReport(md) {
  const report = { title: "", date: "", summary: [], names: new Map(), cohesion: new Map(), god: [], surprising: [], groups: [], commit: null };
  if (!md) return report;
  const head = /^# Graph Report - (.+?)\s+\((.+?)\)/m.exec(md);
  if (head) {
    report.title = head[1];
    report.date = head[2];
  }
  const sections = new Map();
  for (const part of md.split(/^## /m).slice(1)) {
    const nl = part.indexOf("\n");
    sections.set(part.slice(0, nl).trim().toLowerCase(), part.slice(nl + 1));
  }
  const section = (prefix) => [...sections.entries()].find(([name]) => name.startsWith(prefix))?.[1] ?? "";

  report.summary = section("summary")
    .split("\n")
    .filter((l) => l.startsWith("- "))
    .map((l) => l.slice(2).trim());
  report.commit = /Built from commit: `([0-9a-f]+)`/.exec(section("graph freshness"))?.[1] ?? null;
  for (const match of md.matchAll(/^### Community (-?\d+) - "(.*)"\s*\nCohesion: ([\d.]+)/gm)) {
    report.names.set(Number(match[1]), match[2]);
    report.cohesion.set(Number(match[1]), Number(match[3]));
  }
  for (const match of section("god nodes").matchAll(/^\d+\. `(.+)` - (\d+) edges/gm)) report.god.push({ label: match[1], edges: Number(match[2]) });
  const surprise = section("surprising").split("\n");
  for (let i = 0; i < surprise.length; i++) {
    const match = /^- `(.+?)` --(\S+?)--> `(.+?)`\s+\[(\w+)\]/.exec(surprise[i]);
    if (!match) continue;
    const files = /^\s+(.+?) → (.+)$/.exec(surprise[i + 1] ?? "");
    report.surprising.push({ from: match[1], relation: match[2], to: match[3], confidence: match[4], fromFile: files?.[1] ?? "", toFile: files?.[2] ?? "" });
  }
  for (const match of section("hyperedges").matchAll(/^- \*\*(.+?)\*\* — (.+?) \[(\w+)(?: ([\d.]+))?\]/gm)) {
    report.groups.push({ label: match[1], ids: match[2].split(",").map((s) => s.trim()), confidence: match[3] });
  }
  return report;
}

/** Splits the page's one-shot load output into its marked parts. */
function splitMarked(output) {
  const parts = {};
  let key = null;
  let start = 0;
  const re = /^@@([A-Z]+)(?: (.*))?$/gm;
  let match;
  while ((match = re.exec(output))) {
    if (key) parts[key] = output.slice(start, match.index).replace(/\n$/, "");
    key = match[1];
    if (match[2] !== undefined) {
      parts[key] = match[2];
      key = null;
    } else start = match.index + match[0].length + 1;
    if (key === "GRAPH") {
      parts.GRAPH = output.slice(start);
      break;
    }
  }
  if (key && key !== "GRAPH") parts[key] = output.slice(start);
  return parts;
}

/** Repo-relative file paths an agent's terminal mentions, newest last. */
function filesInText(text) {
  const out = [];
  const re = /(?:[\w@.-]+\/)*[\w@.-]+\.(?:tsx?|jsx?|mjs|cjs|rs|py|go|java|kt|swift|c|cc|cpp|h|hpp|rb|php|cs|vue|svelte|md|mdx|json|toml|ya?ml|css|scss|html|sql|sh)\b/g;
  for (const match of String(text).matchAll(re)) out.push(match[0].replace(/^\.\//, ""));
  return out;
}

/** The graph's file for a path seen in a terminal (absolute, worktree-relative or partial). */
function resolveFile(g, path, root) {
  if (root && path.startsWith(root)) path = path.slice(root.length).replace(/^[\\/]/, "");
  if (g.byFile.has(path)) return g.byFile.get(path);
  if (!path.includes("/")) return -1;
  for (const [file, i] of g.byFile) if (path.endsWith(`/${file}`) || file.endsWith(`/${path}`)) return i;
  return -1;
}

// Store ------------------------------------------------------------------------------------------

/** A project's graph facts in one shell call (Unix); parts are marked with @@NAME lines. */
const META_UNIX = [
  `if [ ! -f ${OUT}/graph.json ]; then echo @@NONE; exit 0; fi`,
  `echo "@@STAMP $(date -r ${OUT}/graph.json +%s 2>/dev/null)"`,
  `echo "@@HEAD $(git rev-parse HEAD 2>/dev/null)"`,
  `echo "@@IGNORED $(git check-ignore -q ${OUT} 2>/dev/null; echo $?)"`,
  "echo @@SNAPSHOTS",
  `ls -1 ${OUT}`,
].join("; ");
/** vault.exec keeps only the last 20,000 characters of output; chunks stay under that. */
const CHUNK = 19_000;
/**
 * Keeps the start of a command's output (what an agent needs most) and its exit code, since
 * vault.exec would keep the end.
 */
const headOf = (command, max = CHUNK) => (IS_WIN ? command : `__o=$(${command} 2>&1); __c=$?; printf '%s\n' "$__o" | head -c ${max}; exit $__c`);
const STAMP = IS_WIN
  ? `powershell -NoProfile -Command "if (Test-Path ${OUT}/graph.json) { (Get-Item ${OUT}/graph.json).LastWriteTimeUtc.Ticks }"`
  : `date -r ${OUT}/graph.json +%s 2>/dev/null`;
const UV_INSTALLER = "https://astral.sh/uv/install.sh";
const UV_INSTALLER_WIN = "https://astral.sh/uv/install.ps1";
/** graphify's path (on PATH, else where uv installs tools), version and package folder. */
const DETECT_UNIX = [
  'b=$(command -v graphify 2>/dev/null); onpath=1',
  'if [ -z "$b" ]; then onpath=0; for p in "$HOME/.local/bin/graphify" "$HOME/.local/share/uv/tools/graphifyy/bin/graphify" "$HOME/.local/pipx/venvs/graphifyy/bin/graphify"; do if [ -x "$p" ]; then b="$p"; break; fi; done; fi',
  'if [ -z "$b" ]; then echo @@MISSING; exit 0; fi',
  'echo "@@BIN $b"',
  'echo "@@ONPATH $onpath"',
  'echo "@@VERSION $("$b" --version 2>&1 | head -n 1)"',
  `py=$(head -n 1 "$b" | sed 's/^#! *//')`,
  `echo "@@SKILLS $($py -c 'import graphify, os; print(os.path.dirname(graphify.__file__))' 2>/dev/null)"`,
].join("; ");
const DETECT_WIN =
  'powershell -NoProfile -Command "$c = Get-Command graphify -ErrorAction SilentlyContinue; $b = if ($c) { $c.Source } else { $p = Join-Path $env:USERPROFILE \'.local\\bin\\graphify.exe\'; if (Test-Path $p) { $p } }; if (-not $b) { \'@@MISSING\' } else { \'@@BIN \' + $b; \'@@ONPATH \' + [int][bool]$c; \'@@VERSION \' + (& $b --version | Select-Object -First 1) }"';
const FIND_UV_UNIX = 'command -v uv 2>/dev/null || for p in "$HOME/.local/bin/uv" "$HOME/.cargo/bin/uv"; do if [ -x "$p" ]; then echo "$p"; break; fi; done';
const FIND_UV_WIN = 'where uv 2>nul || if exist "%USERPROFILE%\\.local\\bin\\uv.exe" echo %USERPROFILE%\\.local\\bin\\uv.exe';
/** Runs `command` only if the project has a graph; otherwise prints @@NOGRAPH. */
const ifGraph = (command) =>
  IS_WIN ? `if exist ${OUT}\\graph.json (${command}) else (echo @@NOGRAPH)` : `if [ -f ${OUT}/graph.json ]; then ${command}; else echo @@NOGRAPH; fi`;
const fileExists = (rel) => (IS_WIN ? `if exist ${q(rel.replace(/\//g, "\\"))} (echo @@YES) else (echo @@NO)` : `[ -f ${q(rel)} ] && echo @@YES || echo @@NO`);
const fileUrl = (absolute) => (IS_WIN ? `file:///${encodeURI(absolute.replace(/\\/g, "/"))}` : `file://${encodeURI(absolute)}`);

function snapshotKey(name) {
  const match = SNAPSHOT.exec(name);
  return `${name.slice(0, 10)}-${String(match?.[1] ?? 1).padStart(4, "0")}`;
}

const lastLines = (text, n = 6) => String(text).trim().split("\n").slice(-n).join("\n");

/**
 * What the agent doing a deep map is told. It works for any agent: one with graphify's skill
 * installed runs it; others follow the skill file for their kind of agent from graphify's
 * package (graphify ships one per agent).
 */
function deepMapPrompt(update, gf) {
  const lines = [
    `Build a knowledge graph of this project with graphify: run your graphify skill on "." (in Claude Code that is \`/graphify .${update ? " --update" : ""}\`).`,
    update ? "A graph already exists in graphify-out/, so only re-extract what changed (the skill's --update mode)." : "",
    gf.skillDir
      ? `If you don't have a graphify skill, read graphify's instructions for your kind of agent in ${gf.skillDir} (skill-codex.md for Codex, skill-opencode.md for OpenCode, skill-copilot.md for Copilot, skill.md for Claude Code and others) and follow them for the path ".".`
      : "",
    gf.bin && !gf.onPath ? `The graphify command is at ${gf.bin}.` : "",
    "Write the results to graphify-out/ in this folder, then give a short summary.",
  ];
  return lines.filter(Boolean).join(" ");
}

/** Graphs by project folder, settings, graphify's install state and what agents asked lately. */
function createStore(api) {
  let saved = {};
  try {
    saved = JSON.parse(localStorage.getItem(STORAGE) ?? "{}") ?? {};
  } catch {
    saved = {};
  }
  const settings = { autoUpdate: true, motion: !REDUCED_MOTION, labels: true, flow: true, ...saved.settings };
  const kbs = new Map();
  const listeners = new Set();
  const activityListeners = new Set();
  const activity = [];
  let graphify = { state: "unknown", path: "", version: "" };
  const timers = new Map();

  const changed = () => listeners.forEach((fn) => fn());
  const save = () => localStorage.setItem(STORAGE, JSON.stringify({ settings }));
  let home = null;
  /** The home folder, for workspaces rooted at "~" (vault.exec takes "" as home but not "~"). */
  async function homeDir() {
    if (home === null) {
      const r = await api.vault.exec({ command: IS_WIN ? "echo %USERPROFILE%" : 'echo "$HOME"', cwd: "", keys: [], timeoutSec: 15 });
      home = r.output.trim();
    }
    return home;
  }
  async function absolute(dir) {
    if (dir !== "~" && !dir.startsWith("~/") && !dir.startsWith("~\\")) return dir;
    return (await homeDir()) + dir.slice(1);
  }
  const exec = async (cwd, command, timeoutSec = 120, keys = []) => api.vault.exec({ command, cwd: await absolute(cwd), keys, timeoutSec });
  const anyRoot = () => {
    const s = api.state();
    return s.workspaces.find((w) => w.id === s.activeWorkspaceId)?.rootDir ?? s.workspaces[0]?.rootDir ?? ".";
  };

  function kb(root) {
    let k = kbs.get(root);
    if (!k) {
      k = { root, status: "idle", graph: null, report: null, stamp: "", loadedAt: 0, head: null, ignored: null, snapshots: [], building: null, log: "", error: "", needsForce: false, positions: null, version: 0, loading: null, snapshotIds: new Map() };
      kbs.set(root, k);
    }
    return k;
  }

  /**
   * Finds graphify: on the PATH, or where uv puts it (a fresh install isn't on winfer's PATH
   * until it restarts), plus its version and the folder with its skill files for agents.
   */
  async function detect() {
    graphify = { ...graphify, state: "checking" };
    changed();
    try {
      const r = await exec("", IS_WIN ? DETECT_WIN : DETECT_UNIX, 30);
      const parts = splitMarked(r.output);
      const bin = (parts.BIN ?? "").trim();
      graphify = bin
        ? {
            state: "ok",
            path: bin,
            bin,
            onPath: parts.ONPATH?.trim() === "1",
            version: /graphify v?([\d.]+)/i.exec(parts.VERSION ?? "")?.[1] ?? "",
            skillDir: (parts.SKILLS ?? "").trim(),
          }
        : { state: "missing", path: "", version: "", installError: graphify.installError };
    } catch (err) {
      graphify = { state: "missing", path: "", version: "", error: String(err?.message ?? err) };
    }
    changed();
    return graphify.state === "ok";
  }

  /** The graphify command, quoted, for building shell commands. */
  const bin = () => (graphify.bin ? q(graphify.bin) : "graphify");

  /**
   * Installs graphify with uv, installing uv first when it's missing (with its official
   * installer), or updates it the way it was installed. Progress is in `graphify.step`.
   */
  async function install({ update = false } = {}) {
    const step = (text) => {
      graphify = { ...graphify, state: update ? graphify.state : "installing", step: text, installError: "" };
      changed();
    };
    try {
      // ~/.local/bin/graphify is a link; the package folder tells how it was installed.
      const where = `${graphify.skillDir ?? ""} ${graphify.bin ?? ""}`;
      if (update && !/[\\/]uv[\\/]tools[\\/]/.test(where)) {
        if (!/[\\/]pipx[\\/]/.test(where)) throw new Error(`graphify at ${graphify.bin} wasn't installed with uv or pipx; update it the way you installed it.`);
        step("Updating graphify…");
        const r = await exec("", "pipx upgrade graphifyy", 900);
        if (r.code !== 0) throw new Error(lastLines(r.output, 8));
      } else {
        step("Looking for uv…");
        let uv = (await exec("", IS_WIN ? FIND_UV_WIN : FIND_UV_UNIX, 30)).output.trim().split("\n").pop()?.trim() ?? "";
        if (!uv) {
          step("Installing uv…");
          const r = await exec("", IS_WIN ? `powershell -NoProfile -ExecutionPolicy ByPass -c "irm ${UV_INSTALLER_WIN} | iex"` : `curl -LsSf ${UV_INSTALLER} | sh`, 600);
          if (r.code !== 0) throw new Error(lastLines(r.output, 8) || "Installing uv failed.");
          uv = (await exec("", IS_WIN ? FIND_UV_WIN : FIND_UV_UNIX, 30)).output.trim().split("\n").pop()?.trim() ?? "";
          if (!uv) throw new Error("uv was installed but winfer can't find it. Restart winfer and try again.");
        }
        step(update ? "Updating graphify…" : "Installing graphify…");
        const r = await exec("", `${q(uv)} tool install --upgrade graphifyy`, 900);
        if (r.code !== 0) throw new Error(lastLines(r.output, 8) || "Installing graphify failed.");
      }
      const was = graphify.version;
      graphify = { ...graphify, step: "" };
      if (!(await detect())) throw new Error("graphify was installed but winfer can't find it. Restart winfer and try again.");
      api.toast(update ? (was === graphify.version ? `graphify ${graphify.version} is up to date` : `graphify updated to ${graphify.version}`) : `graphify ${graphify.version} installed`, "success");
      return true;
    } catch (err) {
      const message = String(err?.message ?? err);
      graphify = { ...graphify, state: update ? graphify.state : "missing", step: "", installError: message };
      if (update) api.toast(`Updating graphify failed: ${message}`, "error");
      changed();
      return false;
    }
  }

  function apply(k, parts) {
    const report = parseReport(parts.REPORT ?? "");
    let raw;
    try {
      raw = JSON.parse(parts.GRAPH ?? "");
    } catch {
      throw new Error(`${OUT}/graph.json isn't valid JSON (is a build still writing it?)`);
    }
    k.previousIds = k.graph ? new Set(k.graph.ids) : null;
    k.graph = compactGraph(raw, report.names);
    k.report = report;
    k.stamp = (parts.STAMP ?? "").trim();
    k.loadedAt = Date.now();
    k.builtAt = Number(k.stamp) > 1e9 && Number(k.stamp) < 1e11 ? Number(k.stamp) * 1000 : k.loadedAt;
    k.head = (parts.HEAD ?? "").trim() || null;
    k.ignored = parts.IGNORED?.trim() === "0" ? true : parts.IGNORED?.trim() === "1" ? false : null;
    k.snapshots = (parts.SNAPSHOTS ?? "")
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => SNAPSHOT.test(s))
      .sort((a, b) => snapshotKey(a).localeCompare(snapshotKey(b)));
    k.status = "ready";
    k.error = "";
    k.version++;
  }

  /**
   * Reads a file of the project of any size. vault.exec only returns the last 20,000 characters
   * of output, so the file is gzipped and base64-encoded into a temp file once, read back in
   * chunks (several at a time) and unpacked here.
   */
  async function readText(root, rel) {
    const prepare = IS_WIN
      ? `powershell -NoProfile -Command "$b=[IO.File]::ReadAllBytes((Join-Path (Get-Location) '${rel.replace(/'/g, "''")}')); $m=New-Object IO.MemoryStream; $z=New-Object IO.Compression.GZipStream($m,[IO.Compression.CompressionMode]::Compress); $z.Write($b,0,$b.Length); $z.Close(); $s=[Convert]::ToBase64String($m.ToArray()); $f=[IO.Path]::GetTempFileName(); [IO.File]::WriteAllText($f,$s); Write-Output ([string]$s.Length + ' ' + $f)"`
      : `f=$(mktemp) && gzip -c ${q(rel)} | base64 | tr -d '\\n' > "$f" && echo "$(wc -c < "$f" | tr -d ' ') $f"`;
    const r = await exec(root, prepare, 120);
    const match = /^(\d+) (.+)$/m.exec(r.output.trim());
    if (r.code !== 0 || !match) throw new Error(`Couldn't read ${rel}: ${lastLines(r.output, 3) || `exit code ${r.code}`}`);
    const size = Number(match[1]);
    const file = match[2].trim();
    const chunk = (offset) =>
      IS_WIN
        ? `powershell -NoProfile -Command "[IO.File]::ReadAllText('${file}').Substring(${offset}, ${Math.min(CHUNK, size - offset)})"`
        : `tail -c +${offset + 1} ${q(file)} | head -c ${CHUNK}`;
    try {
      const offsets = [];
      for (let offset = 0; offset < size; offset += CHUNK) offsets.push(offset);
      const parts = new Array(offsets.length);
      let next = 0;
      const worker = async () => {
        while (next < offsets.length) {
          const k = next++;
          const out = await exec(root, chunk(offsets[k]), 60);
          if (out.code !== 0) throw new Error(`Couldn't read ${rel}`);
          parts[k] = out.output.replace(/\s+/g, "");
        }
      };
      await Promise.all(Array.from({ length: Math.min(6, offsets.length) }, worker));
      const b64 = parts.join("");
      if (b64.length !== size) throw new Error(`Reading ${rel} came back incomplete`);
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      return await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"))).text();
    } finally {
      void exec(root, IS_WIN ? `del ${q(file)}` : `rm -f ${q(file)}`, 15).catch(() => {});
    }
  }

  async function metaWindows(root) {
    const has = await exec(root, ifGraph("echo yes"), 30);
    if (has.output.includes("@@NOGRAPH")) return { NONE: "" };
    const [list, head, stamp] = await Promise.all([exec(root, `dir /b ${OUT}`, 30), exec(root, "git rev-parse HEAD", 30), exec(root, STAMP, 30)]);
    return { SNAPSHOTS: list.output, HEAD: head.code === 0 ? head.output : "", STAMP: stamp.output };
  }

  /** (Re)loads a project's graph; concurrent calls share one load. */
  function load(root) {
    const k = kb(root);
    if (k.loading) return k.loading;
    k.loading = (async () => {
      if (!k.graph) k.status = "loading";
      changed();
      try {
        let parts;
        if (IS_WIN) parts = await metaWindows(root);
        else {
          const r = await exec(root, META_UNIX, 60);
          if (r.timedOut) throw new Error("Reading the graph took too long");
          parts = splitMarked(r.output);
        }
        if ("NONE" in parts) {
          k.status = "none";
          k.graph = null;
          k.report = null;
        } else {
          const [report, graph] = await Promise.all([readText(root, `${OUT}/GRAPH_REPORT.md`).catch(() => ""), readText(root, `${OUT}/graph.json`)]);
          apply(k, { ...parts, REPORT: report, GRAPH: graph });
        }
      } catch (err) {
        k.status = k.graph ? "ready" : "error";
        k.error = String(err?.message ?? err);
      } finally {
        k.loading = null;
        changed();
      }
    })();
    return k.loading;
  }

  /** Reloads if graph.json changed on disk; true if it did. */
  async function reloadIfChanged(root) {
    const k = kb(root);
    try {
      const r = await exec(root, STAMP, 30);
      const stamp = r.output.trim();
      if (!stamp || stamp === k.stamp) return false;
    } catch {
      return false;
    }
    await load(root);
    return true;
  }

  /** Node ids of a dated snapshot (graphify-out/<date>/graph.json), for the timeline. */
  async function snapshotIds(root, name) {
    const k = kb(root);
    if (k.snapshotIds.has(name)) return k.snapshotIds.get(name);
    let ids = new Set();
    try {
      ids = new Set((JSON.parse(await readText(root, `${OUT}/${name}/graph.json`)).nodes ?? []).map((node) => String(node.id)));
    } catch {
      // A snapshot without a readable graph shows as empty.
    }
    k.snapshotIds.set(name, ids);
    return ids;
  }

  /**
   * Builds or updates a project's graph:
   * - "quick": `graphify update`, code only (tree-sitter), no AI, seconds.
   * - "agent": the default agent, in a pane in the project folder, runs graphify's skill, which
   *   also reads docs and finds concepts and rationale; the page reloads when that agent finishes.
   * - "headless": `graphify extract --backend claude` with the API key from this plugin's vault.
   */
  async function build(root, mode, { workspaceId, force = false, quiet = false } = {}) {
    const k = kb(root);
    if (k.building) {
      if (!quiet) api.toast("The knowledge graph is already being built");
      return;
    }
    if (mode === "agent") {
      if (workspaceId && api.state().activeWorkspaceId !== workspaceId) api.execute({ type: "workspace.switch", target: { id: workspaceId } });
      api.execute({ type: "pane.spawn", provider: "default", worktree: false, prompt: deepMapPrompt(!!k.graph, graphify) });
      api.execute({ type: "ui.view", view: "workspace" });
      k.building = { mode, startedAt: Date.now(), workspaceId };
      changed();
      return;
    }
    k.building = { mode, startedAt: Date.now() };
    k.needsForce = false;
    changed();
    try {
      const command = mode === "headless" ? `${bin()} extract . --backend claude` : `${bin()} update .${force ? " --force" : ""}`;
      const r = await exec(root, command, mode === "headless" ? 3600 : 900, mode === "headless" ? [API_KEY] : []);
      k.log = r.output;
      if (r.timedOut) throw new Error("graphify took too long and was stopped.");
      if (r.code !== 0) throw new Error(lastLines(r.output) || `graphify exited with code ${r.code}.`);
      k.needsForce = /--force/.test(r.output) && !/Rebuilt:/i.test(r.output);
      k.building = null;
      const before = k.graph?.n ?? 0;
      await load(root);
      if (!quiet && k.graph) {
        const added = k.graph.n - before;
        api.toast(`Knowledge graph ready: ${plural(k.graph.n, "node")}${before && added > 0 ? ` (+${added})` : ""}`, "success");
      }
    } catch (err) {
      k.building = null;
      k.error = String(err?.message ?? err);
      if (!quiet) api.toast("Building the knowledge graph failed", "error");
      changed();
    }
  }

  function cancelBuild(root) {
    const k = kb(root);
    if (k.building?.mode === "agent") {
      k.building = null;
      changed();
    }
  }

  /** After an agent finishes: pick up a finished /graphify run, or refresh the code graph. */
  function agentFinished(root) {
    const k = kbs.get(root);
    if (!k) return;
    if (k.building?.mode === "agent") {
      void reloadIfChanged(root).then((did) => {
        if (did && k.building?.mode === "agent") {
          k.building = null;
          api.toast(`Knowledge graph ready: ${plural(k.graph?.n ?? 0, "node")}`, "success");
          changed();
        }
      });
      return;
    }
    if (!settings.autoUpdate || !k.graph || k.building) return;
    clearTimeout(timers.get(root));
    timers.set(
      root,
      setTimeout(() => {
        timers.delete(root);
        void build(root, "quick", { quiet: true });
      }, 20_000),
    );
  }

  function record(entry) {
    activity.push({ ...entry, at: Date.now() });
    if (activity.length > 40) activity.shift();
    activityListeners.forEach((fn) => fn(entry));
  }

  return {
    settings,
    activity,
    get graphify() {
      return graphify;
    },
    get bin() {
      return bin();
    },
    install,
    kb,
    exec,
    detect,
    load,
    reloadIfChanged,
    snapshotIds,
    readText,
    absolute,
    build,
    cancelBuild,
    agentFinished,
    record,
    set(patch) {
      Object.assign(settings, patch);
      save();
      changed();
    },
    changed,
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    onActivity(fn) {
      activityListeners.add(fn);
      return () => activityListeners.delete(fn);
    },
    dispose() {
      timers.forEach((t) => clearTimeout(t));
    },
  };
}

// Layout -----------------------------------------------------------------------------------------

/**
 * Force layout, run in a worker. Communities sit on the arms of a spiral galaxy (the biggest near
 * the core); nodes are pulled to their community's spot, held apart on a hashed grid (only close
 * nodes push each other, so a tick is O(n)) and pulled together along edges, inside a community
 * more strongly than across. The disc is flattened by pulling harder along y.
 */
const LAYOUT_WORKER = `
"use strict";
const CELL = 22, REPULSE = 9, K_ANCHOR = 0.01, LINK_IN = 14, LINK_OUT = 40, K_IN = 0.35, K_OUT = 0.05, DECAY = 0.6;
let n = 0, m = 0, pos, vel, src, dst, comm, deg, anchor, head, next, mask = 0, alpha = 0, running = false;

function rng(seed) {
  return () => {
    seed |= 0; seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function galaxy(sizes) {
  const C = sizes.length;
  let total = 0;
  for (const s of sizes) total += s;
  const R = 26 * Math.cbrt(total) + 80;
  const out = new Float32Array(C * 3);
  const rand = rng(7);
  for (let r = 0; r < C; r++) {
    const t = C > 1 ? r / (C - 1) : 0;
    const radius = R * (0.04 + 0.96 * Math.sqrt(t));
    const angle = (r % 3) * (2 * Math.PI / 3) + (radius / R) * 4.4 + (rand() - 0.5) * 0.5;
    out[r * 3] = Math.cos(angle) * radius;
    out[r * 3 + 1] = (rand() - 0.5) * R * 0.1 * (1.2 - t);
    out[r * 3 + 2] = Math.sin(angle) * radius;
  }
  return { anchors: out, R };
}

const hash = (x, y, z) => (Math.imul(x, 73856093) ^ Math.imul(y, 19349663) ^ Math.imul(z, 83492791)) & mask;

function tick() {
  for (let i = 0; i < n; i++) {
    const a = comm[i] * 3, p = i * 3;
    vel[p] += (anchor[a] - pos[p]) * K_ANCHOR * alpha;
    vel[p + 1] += (anchor[a + 1] - pos[p + 1]) * K_ANCHOR * 3 * alpha;
    vel[p + 2] += (anchor[a + 2] - pos[p + 2]) * K_ANCHOR * alpha;
  }
  for (let e = 0; e < m; e++) {
    const a = src[e], b = dst[e], pa = a * 3, pb = b * 3;
    let dx = pos[pb] + vel[pb] - pos[pa] - vel[pa];
    let dy = pos[pb + 1] + vel[pb + 1] - pos[pa + 1] - vel[pa + 1];
    let dz = pos[pb + 2] + vel[pb + 2] - pos[pa + 2] - vel[pa + 2];
    const d = Math.sqrt(dx * dx + dy * dy + dz * dz) || 1e-3;
    const same = comm[a] === comm[b];
    const l = ((d - (same ? LINK_IN : LINK_OUT)) / d) * alpha * (same ? K_IN : K_OUT) / Math.min(deg[a], deg[b]);
    dx *= l; dy *= l; dz *= l;
    const bias = deg[a] / (deg[a] + deg[b]);
    vel[pb] -= dx * bias; vel[pb + 1] -= dy * bias; vel[pb + 2] -= dz * bias;
    vel[pa] += dx * (1 - bias); vel[pa + 1] += dy * (1 - bias); vel[pa + 2] += dz * (1 - bias);
  }
  head.fill(-1);
  for (let i = 0; i < n; i++) {
    const h = hash(Math.floor(pos[i * 3] / CELL), Math.floor(pos[i * 3 + 1] / CELL), Math.floor(pos[i * 3 + 2] / CELL));
    next[i] = head[h];
    head[h] = i;
  }
  const c2 = CELL * CELL;
  for (let i = 0; i < n; i++) {
    const p = i * 3;
    const cx = Math.floor(pos[p] / CELL), cy = Math.floor(pos[p + 1] / CELL), cz = Math.floor(pos[p + 2] / CELL);
    for (let ox = -1; ox <= 1; ox++) for (let oy = -1; oy <= 1; oy++) for (let oz = -1; oz <= 1; oz++) {
      for (let j = head[hash(cx + ox, cy + oy, cz + oz)]; j !== -1; j = next[j]) {
        if (j <= i) continue;
        const q = j * 3;
        let x = pos[p] - pos[q], y = pos[p + 1] - pos[q + 1], z = pos[p + 2] - pos[q + 2];
        let d2 = x * x + y * y + z * z;
        if (d2 >= c2) continue;
        if (d2 < 1e-6) { x = (i % 7) * 0.01 + 0.01; y = (j % 5) * 0.01; z = 0.01; d2 = x * x + y * y + z * z; }
        const d = Math.sqrt(d2);
        const f = (REPULSE * (1 - d / CELL) * (1 - d / CELL) / d) * alpha;
        vel[p] += x * f; vel[p + 1] += y * f; vel[p + 2] += z * f;
        vel[q] -= x * f; vel[q + 1] -= y * f; vel[q + 2] -= z * f;
      }
    }
  }
  for (let k = 0; k < n * 3; k++) {
    vel[k] *= DECAY;
    pos[k] += vel[k];
  }
  alpha += -alpha * 0.018;
}

function loop() {
  if (!running) return;
  const start = performance.now();
  let ticks = 0;
  do { tick(); ticks++; } while (performance.now() - start < 10 && ticks < 3);
  const out = pos.slice();
  postMessage({ type: "positions", positions: out, alpha }, [out.buffer]);
  if (alpha < 0.004) { running = false; postMessage({ type: "settled" }); return; }
  setTimeout(loop, 12);
}

onmessage = (event) => {
  const msg = event.data;
  if (msg.type === "init") {
    n = msg.n; m = msg.src.length; src = msg.src; dst = msg.dst; comm = msg.comm; deg = msg.deg;
    const g = galaxy(msg.sizes);
    anchor = g.anchors;
    pos = msg.positions ?? new Float32Array(n * 3).fill(NaN);
    vel = new Float32Array(n * 3);
    let size = 1024;
    while (size < n * 4) size <<= 1;
    mask = size - 1;
    head = new Int32Array(size);
    next = new Int32Array(n);
    const rand = rng(1234);
    const gauss = () => (rand() + rand() + rand() - 1.5) * 1.4;
    for (let i = 0; i < n; i++) {
      if (!Number.isNaN(pos[i * 3])) continue;
      const a = comm[i] * 3, spread = CELL * 0.8 * Math.cbrt(msg.sizes[comm[i]]) + 3;
      pos[i * 3] = anchor[a] + gauss() * spread;
      pos[i * 3 + 1] = anchor[a + 1] + gauss() * spread * 0.5;
      pos[i * 3 + 2] = anchor[a + 2] + gauss() * spread;
    }
    alpha = msg.alpha;
    postMessage({ type: "meta", R: g.R });
    const first = pos.slice();
    postMessage({ type: "positions", positions: first, alpha }, [first.buffer]);
    if (!running) { running = true; loop(); }
  } else if (msg.type === "reheat") {
    alpha = Math.max(alpha, msg.alpha);
    if (!running && n) { running = true; loop(); }
  } else if (msg.type === "stop") {
    running = false;
  }
};
`;

// Rendering --------------------------------------------------------------------------------------

const NODE_VS = `#version 300 es
layout(location=0) in vec2 aCorner;
layout(location=1) in vec3 aPos;
layout(location=2) in vec4 aColor;
layout(location=3) in vec4 aState;
layout(location=4) in vec3 aPulse;
uniform mat4 uView, uProj;
uniform float uTime, uViewportH, uScan, uScanR;
out vec2 vUv; out vec4 vColor; out float vShape; out float vPulse; out vec3 vPulseColor; out float vEmph;
void main() {
  float size = aState.x;
  vec4 vp = uView * vec4(aPos, 1.0);
  float pulse = 0.0;
  if (aState.z > 0.0) { float t = uTime - aState.z; pulse = (t >= 0.0 && t < 2.2) ? 1.0 - t / 2.2 : 0.0; }
  float scan = uScan * exp(-pow((length(aPos.xz) - uScanR) / 26.0, 2.0));
  float s = size * (aState.y > 1.5 ? 1.5 : 1.0) * (1.0 + pulse * 1.3 + scan * 0.9);
  float px = s * uProj[1][1] * uViewportH * 0.5 / max(-vp.z, 0.001);
  s *= max(1.0, 1.7 / max(px, 1e-4));
  vp.xy += aCorner * s;
  gl_Position = size > 0.0 ? uProj * vp : vec4(2.0, 2.0, 2.0, 1.0);
  vUv = aCorner; vColor = aColor; vShape = aState.w; vEmph = aState.y;
  vPulse = max(pulse, scan * 0.7);
  vPulseColor = pulse > 0.0 ? aPulse : vec3(1.0);
}`;

const NODE_FS = `#version 300 es
precision highp float;
in vec2 vUv; in vec4 vColor; in float vShape; in float vPulse; in vec3 vPulseColor; in float vEmph;
uniform float uLight;
out vec4 outColor;
void main() {
  float r = length(vUv);
  if (r > 1.0) discard;
  float halo = pow(1.0 - r, 2.6) * 0.6;
  float core;
  if (vShape < 0.5) core = smoothstep(0.36, 0.17, r);
  else if (vShape < 1.5) core = smoothstep(0.08, 0.0, abs(r - 0.27)) + smoothstep(0.12, 0.0, r) * 0.7;
  else if (vShape < 2.5) {
    vec2 a = abs(vUv);
    core = exp(-a.x * 16.0) * exp(-a.y * 2.6) + exp(-a.y * 16.0) * exp(-a.x * 2.6) + smoothstep(0.2, 0.05, r);
  } else if (vShape < 3.5) {
    core = smoothstep(0.045, 0.0, abs(r - 0.88));
    halo = smoothstep(0.9, 0.55, r) * smoothstep(0.35, 0.88, r) * 0.14;
  } else {
    core = smoothstep(0.07, 0.0, abs(r - 0.62)) + smoothstep(0.27, 0.1, r);
    halo *= 0.9;
  }
  float ring = vPulse > 0.0 ? smoothstep(0.08, 0.0, abs(r - (1.0 - vPulse) * 0.95)) * vPulse : 0.0;
  vec3 base = mix(vColor.rgb, vPulseColor, clamp(ring * 1.4 + vPulse * 0.4, 0.0, 1.0));
  float alpha = vColor.a;
  if (uLight > 0.5) {
    float a = clamp(core + halo * 0.5 + ring, 0.0, 1.0) * alpha;
    outColor = vec4(base * a, a);
  } else {
    vec3 c = base * (halo + core * 0.9 + ring) + vec3(core * (vEmph > 1.5 ? 0.5 : 0.16));
    c *= alpha;
    outColor = vec4(c, clamp(max(max(c.r, c.g), c.b), 0.0, 1.0));
  }
}`;

const EDGE_VS = `#version 300 es
layout(location=0) in vec3 aPos;
layout(location=1) in vec4 aColor;
layout(location=2) in vec2 aT;
uniform mat4 uView, uProj;
out vec4 vColor; out vec2 vT;
void main() { gl_Position = uProj * uView * vec4(aPos, 1.0); vColor = aColor; vT = aT; }`;

const EDGE_FS = `#version 300 es
precision highp float;
in vec4 vColor; in vec2 vT;
out vec4 outColor;
void main() {
  if (vT.y > 0.5 && fract(vT.x * 10.0) > 0.5) discard;
  outColor = vec4(vColor.rgb * vColor.a, vColor.a);
}`;

const PARTICLE_VS = `#version 300 es
layout(location=0) in vec2 aCorner;
layout(location=1) in vec3 aA;
layout(location=2) in vec3 aB;
layout(location=3) in vec4 aColor;
layout(location=4) in vec4 aParam;
uniform mat4 uView, uProj;
uniform float uTime, uViewportH;
out vec2 vUv; out vec4 vColor;
void main() {
  float t = fract(aParam.x + uTime * aParam.y);
  vec4 vp = uView * vec4(mix(aA, aB, t), 1.0);
  float s = aParam.w;
  float px = s * uProj[1][1] * uViewportH * 0.5 / max(-vp.z, 0.001);
  s *= max(1.0, 1.3 / max(px, 1e-4));
  vp.xy += aCorner * s;
  gl_Position = uProj * vp;
  vUv = aCorner;
  vColor = vec4(aColor.rgb, aColor.a * aParam.z * sin(t * 3.14159));
}`;

const PARTICLE_FS = `#version 300 es
precision highp float;
in vec2 vUv; in vec4 vColor;
out vec4 outColor;
void main() {
  float r = length(vUv);
  if (r > 1.0) discard;
  float a = pow(1.0 - r, 2.0) * vColor.a;
  outColor = vec4(vColor.rgb * a, a);
}`;

const STAR_VS = `#version 300 es
layout(location=0) in vec3 aPos;
layout(location=1) in float aSize;
uniform mat4 uRot, uProj;
uniform float uTime, uDpr;
out float vA;
void main() {
  gl_Position = uProj * (uRot * vec4(aPos * 2400.0, 1.0));
  gl_PointSize = aSize * uDpr;
  vA = 0.35 + 0.65 * (0.5 + 0.5 * sin(uTime * (0.5 + aSize) + aPos.x * 40.0));
}`;

const STAR_FS = `#version 300 es
precision highp float;
in float vA;
out vec4 outColor;
void main() {
  vec2 c = gl_PointCoord * 2.0 - 1.0;
  float r = dot(c, c);
  if (r > 1.0) discard;
  float a = (1.0 - r) * vA * 0.6;
  outColor = vec4(vec3(0.78, 0.82, 1.0) * a, a);
}`;

function perspective(fovy, aspect, near, far) {
  const f = 1 / Math.tan(fovy / 2);
  const nf = 1 / (near - far);
  return new Float32Array([f / aspect, 0, 0, 0, 0, f, 0, 0, 0, 0, (far + near) * nf, -1, 0, 0, 2 * far * near * nf, 0]);
}

function lookAt(eye, target) {
  let zx = eye[0] - target[0], zy = eye[1] - target[1], zz = eye[2] - target[2];
  let len = Math.hypot(zx, zy, zz) || 1;
  zx /= len; zy /= len; zz /= len;
  // x = up × z with up = +y
  let xx = zz, xy = 0, xz = -zx;
  len = Math.hypot(xx, xy, xz) || 1;
  xx /= len; xy /= len; xz /= len;
  const yx = zy * xz - zz * xy, yy = zz * xx - zx * xz, yz = zx * xy - zy * xx;
  return new Float32Array([
    xx, yx, zx, 0,
    xy, yy, zy, 0,
    xz, yz, zz, 0,
    -(xx * eye[0] + xy * eye[1] + xz * eye[2]), -(yx * eye[0] + yy * eye[1] + yz * eye[2]), -(zx * eye[0] + zy * eye[1] + zz * eye[2]), 1,
  ]);
}

function multiply(a, b) {
  const out = new Float32Array(16);
  for (let c = 0; c < 4; c++) for (let r = 0; r < 4; r++) {
    out[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
  }
  return out;
}

const easeInOut = (t) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);
const easeOut = (t) => 1 - (1 - t) ** 3;

function program(gl, vs, fs) {
  const shader = (type, source) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, source);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader failed to compile");
    return s;
  };
  const p = gl.createProgram();
  gl.attachShader(p, shader(gl.VERTEX_SHADER, vs));
  gl.attachShader(p, shader(gl.FRAGMENT_SHADER, fs));
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) ?? "shader failed to link");
  const uniforms = {};
  const count = gl.getProgramParameter(p, gl.ACTIVE_UNIFORMS);
  for (let i = 0; i < count; i++) {
    const name = gl.getActiveUniform(p, i).name;
    uniforms[name] = gl.getUniformLocation(p, name);
  }
  return { p, u: uniforms };
}

/**
 * The 3D galaxy: WebGL2 (instanced glow sprites, additive lines, particles flowing along edges,
 * a starfield) or a plain 2D canvas where WebGL2 isn't available. Owns the camera and the layout
 * worker; the page tells it what to show (focus, filters, path, pulses, agents).
 */
function createGalaxy(container, callbacks) {
  const canvas = h("canvas", { class: "kb-canvas" });
  const labelLayer = h("div", { class: "kb-labels" });
  container.append(canvas, labelLayer);

  let gl = canvas.getContext("webgl2", { antialias: true, premultipliedAlpha: true, alpha: true });
  const ctx2d = gl ? null : canvas.getContext("2d");
  let res = null;
  let lost = false;

  const cam = { target: [0, 0, 0], yaw: 0.7, pitch: 0.62, dist: 900, anim: null };
  const fov = (48 * Math.PI) / 180;
  let view = new Float32Array(16);
  let proj = new Float32Array(16);
  let viewProj = new Float32Array(16);
  let width = 1;
  let height = 1;
  let dpr = 1;

  let g = null;
  let pos = null;
  let R = 300;
  let worker = null;
  let settled = true;
  let light = document.documentElement.dataset.mode === "light";
  let options = { motion: true, flow: true, labels: true };

  let baseColor = new Float32Array(0);
  let nodeSize = new Float32Array(0);
  let shape = new Float32Array(0);
  let visible = new Uint8Array(0);
  let presence = null;
  let confVisible = [true, true, true];
  let focus = null;
  let keys = new Set();
  let hover = -1;
  let selected = -1;
  let path = [];
  let pathEdges = new Set();
  let pulseStart = new Float32Array(0);
  let pulseColor = new Float32Array(0);
  let rings = [];
  let agents = [];
  let scanOn = false;
  let scanLevel = 0;
  let topNodes = [];
  let centroids = [];
  let centroidTick = 0;

  // CPU-side buffers, uploaded when their inputs change.
  let nodeColorBuf = new Float32Array(0);
  let nodeStateBuf = new Float32Array(0);
  let edgePosBuf = new Float32Array(0);
  let edgeColorBuf = new Float32Array(0);
  let edgeTBuf = new Float32Array(0);
  let flow = { edges: new Int32Array(0), dirs: new Int8Array(0), color: new Float32Array(0), param: new Float32Array(0), ab: new Float32Array(0) };
  let positionsDirty = false;
  let styleDirty = false;
  let flowDirty = false;
  let pulseDirty = false;
  let paused = false;
  let lastInteract = performance.now();
  let loadedAt = 0;
  let lastFrame = performance.now();
  let raf = 0;
  const t0 = performance.now();
  const now = () => (performance.now() - t0) / 1000;

  // Labels: a pool of absolutely positioned elements moved every frame.
  const labelPool = [];
  let labelItems = [];

  // WebGL resources ----------------------------------------------------------------------------

  function initGL() {
    if (!gl) return;
    const quad = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, quad);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    const attrib = (loc, buf, size, divisor = 0) => {
      gl.bindBuffer(gl.ARRAY_BUFFER, buf);
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, size, gl.FLOAT, false, 0, 0);
      gl.vertexAttribDivisor(loc, divisor);
    };
    const sprites = () => {
      const b = { pos: gl.createBuffer(), color: gl.createBuffer(), state: gl.createBuffer(), pulse: gl.createBuffer() };
      b.vao = gl.createVertexArray();
      gl.bindVertexArray(b.vao);
      attrib(0, quad, 2);
      attrib(1, b.pos, 3, 1);
      attrib(2, b.color, 4, 1);
      attrib(3, b.state, 4, 1);
      attrib(4, b.pulse, 3, 1);
      return b;
    };
    const lines = () => {
      const b = { pos: gl.createBuffer(), color: gl.createBuffer(), t: gl.createBuffer() };
      b.vao = gl.createVertexArray();
      gl.bindVertexArray(b.vao);
      attrib(0, b.pos, 3);
      attrib(1, b.color, 4);
      attrib(2, b.t, 2);
      return b;
    };
    const particles = { ab: gl.createBuffer(), color: gl.createBuffer(), param: gl.createBuffer(), vao: gl.createVertexArray() };
    gl.bindVertexArray(particles.vao);
    attrib(0, quad, 2);
    gl.bindBuffer(gl.ARRAY_BUFFER, particles.ab);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 3, gl.FLOAT, false, 24, 0);
    gl.vertexAttribDivisor(1, 1);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 3, gl.FLOAT, false, 24, 12);
    gl.vertexAttribDivisor(2, 1);
    attrib(3, particles.color, 4, 1);
    attrib(4, particles.param, 4, 1);

    const stars = { pos: gl.createBuffer(), size: gl.createBuffer(), vao: gl.createVertexArray(), count: 1400 };
    const rand = mulberry32(99);
    const starPos = new Float32Array(stars.count * 3);
    const starSize = new Float32Array(stars.count);
    for (let i = 0; i < stars.count; i++) {
      const u = rand() * 2 - 1;
      const t = rand() * Math.PI * 2;
      const s = Math.sqrt(1 - u * u);
      starPos.set([s * Math.cos(t), u, s * Math.sin(t)], i * 3);
      starSize[i] = 0.6 + rand() ** 3 * 2.4;
    }
    gl.bindVertexArray(stars.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, stars.pos);
    gl.bufferData(gl.ARRAY_BUFFER, starPos, gl.STATIC_DRAW);
    attrib(0, stars.pos, 3);
    gl.bindBuffer(gl.ARRAY_BUFFER, stars.size);
    gl.bufferData(gl.ARRAY_BUFFER, starSize, gl.STATIC_DRAW);
    attrib(1, stars.size, 1);
    gl.bindVertexArray(null);

    res = {
      node: program(gl, NODE_VS, NODE_FS),
      edge: program(gl, EDGE_VS, EDGE_FS),
      particle: program(gl, PARTICLE_VS, PARTICLE_FS),
      star: program(gl, STAR_VS, STAR_FS),
      nodes: sprites(),
      fx: sprites(),
      edges: lines(),
      beams: lines(),
      particles,
      stars,
    };
    positionsDirty = styleDirty = flowDirty = true;
  }

  try {
    initGL();
  } catch (err) {
    console.warn("knowledge-base: WebGL2 failed, using 2D", err);
    gl = null;
  }
  const fallback = gl ? null : ctx2d ?? canvas.getContext("2d");

  canvas.addEventListener("webglcontextlost", (e) => {
    e.preventDefault();
    lost = true;
  });
  canvas.addEventListener("webglcontextrestored", () => {
    lost = false;
    initGL();
  });

  const upload = (buf, data, usage) => {
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, data, usage ?? gl.DYNAMIC_DRAW);
  };

  // Camera -------------------------------------------------------------------------------------

  function eye() {
    const cp = Math.cos(cam.pitch);
    return [cam.target[0] + cam.dist * cp * Math.sin(cam.yaw), cam.target[1] + cam.dist * Math.sin(cam.pitch), cam.target[2] + cam.dist * cp * Math.cos(cam.yaw)];
  }

  function updateMatrices() {
    view = lookAt(eye(), cam.target);
    proj = perspective(fov, width / height, Math.max(0.5, cam.dist * 0.01), Math.max(cam.dist * 8, 6000));
    viewProj = multiply(proj, view);
  }

  function project(x, y, z) {
    const m = viewProj;
    const w = m[3] * x + m[7] * y + m[11] * z + m[15];
    if (w <= 0.01) return null;
    const cx = (m[0] * x + m[4] * y + m[8] * z + m[12]) / w;
    const cy = (m[1] * x + m[5] * y + m[9] * z + m[13]) / w;
    return { x: (cx * 0.5 + 0.5) * width, y: (0.5 - cy * 0.5) * height, w };
  }

  function flyTo(target, dist, ms = 1100) {
    if (ms <= 0) {
      cam.target = [...target];
      cam.dist = dist;
      cam.anim = null;
      return;
    }
    cam.anim = { from: [...cam.target], fromDist: cam.dist, to: target, toDist: dist, start: performance.now(), ms };
    lastInteract = performance.now();
  }

  function boundsOf(indices) {
    if (!pos || !indices.length) return { c: [0, 0, 0], r: R };
    let cx = 0, cy = 0, cz = 0;
    for (const i of indices) {
      cx += pos[i * 3];
      cy += pos[i * 3 + 1];
      cz += pos[i * 3 + 2];
    }
    cx /= indices.length;
    cy /= indices.length;
    cz /= indices.length;
    const d = indices.map((i) => Math.hypot(pos[i * 3] - cx, pos[i * 3 + 1] - cy, pos[i * 3 + 2] - cz)).sort((a, b) => a - b);
    return { c: [cx, cy, cz], r: Math.max(40, d[Math.floor((d.length - 1) * 0.94)] ?? 40) };
  }

  const distFor = (radius) => (radius / Math.tan(fov / 2)) * 1.2 * Math.max(1, height / width);

  function fitIndices(indices, ms) {
    const b = boundsOf(indices);
    flyTo(b.c, clamp(distFor(b.r), 40, 9000), ms);
  }

  function fit(ms = 1200) {
    if (!g) return flyTo([0, 0, 0], distFor(R), ms);
    const list = [];
    for (let i = 0; i < g.n; i++) if (visible[i]) list.push(i);
    fitIndices(list, ms);
  }

  // Data ---------------------------------------------------------------------------------------

  function setGraph(graph, cached, fresh) {
    g = graph;
    const n = g.n;
    nodeSize = new Float32Array(n);
    shape = new Float32Array(n);
    visible = new Uint8Array(n).fill(1);
    pulseStart = new Float32Array(n);
    pulseColor = new Float32Array(n * 3).fill(1);
    nodeColorBuf = new Float32Array(n * 4);
    nodeStateBuf = new Float32Array(n * 4);
    edgePosBuf = new Float32Array(g.m * 6);
    edgeColorBuf = new Float32Array(g.m * 8);
    edgeTBuf = new Float32Array(g.m * 4);
    for (let i = 0; i < n; i++) {
      nodeSize[i] = Math.min(10, 1.3 + Math.sqrt(g.degree[i]) * 0.6);
      const t = g.types[i];
      shape[i] = t === "document" || t === "paper" ? 1 : t === "concept" || t === "rationale" ? 2 : 0;
    }
    for (let e = 0; e < g.m; e++) {
      edgeTBuf[e * 4 + 2] = 1;
      edgeTBuf[e * 4 + 1] = edgeTBuf[e * 4 + 3] = g.conf[e] === 2 ? 1 : 0;
    }
    topNodes = [...Array(n).keys()].sort((a, b) => g.degree[b] - g.degree[a]).slice(0, 12);
    recolor();
    focus = null;
    keys = new Set();
    hover = selected = -1;
    path = [];
    pathEdges = new Set();
    presence = null;

    pos = cached ?? null;
    loadedAt = performance.now();
    worker?.terminate();
    worker = new Worker(URL.createObjectURL(new Blob([LAYOUT_WORKER], { type: "text/javascript" })));
    const sizes = Int32Array.from(g.communities.map((c) => c.count));
    worker.onmessage = (event) => {
      const msg = event.data;
      if (msg.type === "meta") R = msg.R;
      else if (msg.type === "positions") {
        const first = !pos || pos.length !== msg.positions.length;
        pos = msg.positions;
        positionsDirty = true;
        settled = false;
        if (first) fit(0);
      } else if (msg.type === "settled") {
        settled = true;
        // Frame the finished layout unless the user already moved the camera.
        if (lastInteract < loadedAt) fit(1400);
      }
    };
    const hasCache = !!cached && cached.length === n * 3;
    worker.postMessage({
      type: "init",
      n,
      src: g.src,
      dst: g.dst,
      comm: g.commRank,
      deg: Int32Array.from(g.degree, (d) => Math.max(1, d)),
      sizes,
      positions: hasCache ? cached.slice() : null,
      alpha: hasCache ? (fresh?.size ? 0.25 : 0.05) : 1,
    });
    if (fresh?.size) pulse([...fresh].map((id) => g.index.get(id)).filter((i) => i !== undefined), [0.6, 1, 0.8], 0.6);
    styleDirty = flowDirty = true;
    if (!hasCache) {
      cam.yaw = 0.7;
      cam.pitch = 0.62;
    }
  }

  function recolor() {
    if (!g) return;
    baseColor = new Float32Array(g.n * 3);
    for (let i = 0; i < g.n; i++) baseColor.set(communityColor(g.commRank[i], light), i * 3);
    styleDirty = flowDirty = true;
  }

  function computeStyle() {
    const n = g.n;
    const hoverSet = hover >= 0 && !path.length ? new Set([hover, ...neighbors(g, hover)]) : null;
    const f = hoverSet ?? focus;
    const dim = light ? 0.13 : 0.1;
    for (let i = 0; i < n; i++) {
      const vis = visible[i] && (!presence || presence[i]);
      const inF = f ? f.has(i) : true;
      const key = i === selected || i === hover || keys.has(i);
      const emph = !vis ? 0 : f ? (inF ? (key ? 2 : 1) : 0.05) : key ? 2 : 1;
      nodeStateBuf[i * 4] = vis ? nodeSize[i] : 0;
      nodeStateBuf[i * 4 + 1] = emph;
      nodeStateBuf[i * 4 + 2] = pulseStart[i];
      nodeStateBuf[i * 4 + 3] = shape[i];
      nodeColorBuf[i * 4] = baseColor[i * 3];
      nodeColorBuf[i * 4 + 1] = baseColor[i * 3 + 1];
      nodeColorBuf[i * 4 + 2] = baseColor[i * 3 + 2];
      nodeColorBuf[i * 4 + 3] = emph >= 1 ? 1 : dim;
    }
    const warn = light ? [0.8, 0.5, 0.05] : [1, 0.75, 0.3];
    const base = light ? [0.2, 0.13, 0.28] : [0.11, 0.07, 0.2];
    for (let e = 0; e < g.m; e++) {
      const a = g.src[e];
      const b = g.dst[e];
      const vis = visible[a] && visible[b] && confVisible[g.conf[e]] && (!presence || (presence[a] && presence[b]));
      let alpha = 0;
      if (vis) {
        if (pathEdges.has(e)) alpha = 1;
        else if (f) alpha = f.has(a) && f.has(b) ? (light ? 0.55 : 0.5) : light ? 0.02 : 0.012;
        else alpha = base[g.conf[e]];
      }
      const o = e * 8;
      if (g.conf[e] === 2) {
        edgeColorBuf.set(warn, o);
        edgeColorBuf.set(warn, o + 4);
      } else if (pathEdges.has(e)) {
        edgeColorBuf.set([1, 1, 1], o);
        edgeColorBuf.set([1, 1, 1], o + 4);
      } else {
        edgeColorBuf[o] = baseColor[a * 3];
        edgeColorBuf[o + 1] = baseColor[a * 3 + 1];
        edgeColorBuf[o + 2] = baseColor[a * 3 + 2];
        edgeColorBuf[o + 4] = baseColor[b * 3];
        edgeColorBuf[o + 5] = baseColor[b * 3 + 1];
        edgeColorBuf[o + 6] = baseColor[b * 3 + 2];
      }
      edgeColorBuf[o + 3] = edgeColorBuf[o + 7] = alpha;
    }
    labelItems = labelCandidates(f);
  }

  /** Which edges carry particles: comets along a path, all focused edges, or a calm ambient flow. */
  function computeFlow() {
    const edges = [];
    const dirs = [];
    const colors = [];
    const params = [];
    const hoverSet = hover >= 0 && !path.length ? new Set([hover, ...neighbors(g, hover)]) : null;
    const f = hoverSet ?? focus;
    const add = (e, dir, color, alpha, phase, speed, size) => {
      edges.push(e);
      dirs.push(dir);
      colors.push(color[0], color[1], color[2], 1);
      params.push(phase, speed, alpha, size);
    };
    const white = light ? [0.15, 0.15, 0.3] : [1, 1, 1];
    if (path.length > 1) {
      for (let k = 0; k + 1 < path.length; k++) {
        const e = edgeBetween(g, path[k], path[k + 1]);
        if (e < 0) continue;
        const dir = g.src[e] === path[k] ? 1 : -1;
        for (let j = 0; j < 9; j++) add(e, dir, white, 1 - j / 9, k * 0.12 - j * 0.016, 0.42, 1.9 - j * 0.12);
      }
    }
    if (options.flow || f) {
      let count = 0;
      const cap = f ? 2400 : 1800;
      const stride = f ? 1 : Math.max(1, Math.ceil(g.m / 6000));
      for (let e = 0; e < g.m && count < cap; e += stride) {
        const a = g.src[e];
        const b = g.dst[e];
        if (!visible[a] || !visible[b] || !confVisible[g.conf[e]]) continue;
        if (presence && !(presence[a] && presence[b])) continue;
        if (f ? !(f.has(a) && f.has(b)) : !FLOW_RELATIONS.has(g.relations[g.rel[e]])) continue;
        if (pathEdges.has(e)) continue;
        const r = (hashString(String(e)) % 1000) / 1000;
        const color = [baseColor[b * 3], baseColor[b * 3 + 1], baseColor[b * 3 + 2]];
        if (f) {
          add(e, 1, color, 0.95, r, 0.22 + r * 0.2, 1.1);
          add(e, 1, color, 0.95, r + 0.5, 0.22 + r * 0.2, 1.1);
        } else add(e, 1, color, light ? 0.5 : 0.65, r, 0.06 + r * 0.1, 0.9);
        count++;
      }
    }
    flow = {
      edges: Int32Array.from(edges),
      dirs: Int8Array.from(dirs),
      color: Float32Array.from(colors),
      param: Float32Array.from(params),
      ab: new Float32Array(edges.length * 6),
    };
    positionsDirty = true;
  }

  function fillPositions() {
    for (let e = 0; e < g.m; e++) {
      const a = g.src[e] * 3;
      const b = g.dst[e] * 3;
      const o = e * 6;
      edgePosBuf[o] = pos[a];
      edgePosBuf[o + 1] = pos[a + 1];
      edgePosBuf[o + 2] = pos[a + 2];
      edgePosBuf[o + 3] = pos[b];
      edgePosBuf[o + 4] = pos[b + 1];
      edgePosBuf[o + 5] = pos[b + 2];
    }
    for (let k = 0; k < flow.edges.length; k++) {
      const e = flow.edges[k];
      const [from, to] = flow.dirs[k] > 0 ? [g.src[e], g.dst[e]] : [g.dst[e], g.src[e]];
      flow.ab.set([pos[from * 3], pos[from * 3 + 1], pos[from * 3 + 2], pos[to * 3], pos[to * 3 + 1], pos[to * 3 + 2]], k * 6);
    }
  }

  function computeCentroids() {
    const sums = new Map();
    for (let i = 0; i < g.n; i++) {
      if (!visible[i]) continue;
      const r = g.commRank[i];
      if (r > 7) continue;
      const s = sums.get(r) ?? [0, 0, 0, 0];
      s[0] += pos[i * 3];
      s[1] += pos[i * 3 + 1];
      s[2] += pos[i * 3 + 2];
      s[3]++;
      sums.set(r, s);
    }
    centroids = [...sums.entries()].filter(([, s]) => s[3] >= 3).map(([rank, s]) => ({ rank, p: [s[0] / s[3], s[1] / s[3] + 10, s[2] / s[3]] }));
  }

  // Labels -------------------------------------------------------------------------------------

  function labelCandidates(f) {
    const items = [];
    const seen = new Set();
    const add = (i, kind) => {
      if (i < 0 || seen.has(i) || !visible[i]) return;
      seen.add(i);
      items.push({ i, kind });
    };
    add(selected, "selected");
    path.forEach((i) => add(i, "path"));
    keys.forEach((i) => add(i, "key"));
    if (f) {
      [...f]
        .filter((i) => !seen.has(i))
        .sort((a, b) => g.degree[b] - g.degree[a])
        .slice(0, 16)
        .forEach((i) => add(i, "near"));
    } else if (options.labels) topNodes.forEach((i) => add(i, "top"));
    return items;
  }

  function labelEl(k) {
    while (labelPool.length <= k) {
      const el = h("div", { class: "kb-label" });
      labelLayer.append(el);
      labelPool.push(el);
    }
    return labelPool[k];
  }

  function drawLabels() {
    let k = 0;
    // Greedy overlap avoidance: labels are placed in priority order and optional ones that would
    // cover an earlier label are skipped (rough size from the text length).
    const placed = [];
    const show = (p, text, cls, opacity, color, optional = false) => {
      const s = project(p[0], p[1], p[2]);
      if (!s || s.x < -100 || s.y < -40 || s.x > width + 100 || s.y > height + 40) return;
      const w = Math.min(260, text.length * 6.4 + 16);
      const box = [s.x - w / 2, s.y - 20, s.x + w / 2, s.y];
      if (optional && placed.some((b) => box[0] < b[2] && box[2] > b[0] && box[1] < b[3] && box[3] > b[1])) return;
      placed.push(box);
      const el = labelEl(k++);
      if (el.dataset.text !== text) {
        el.dataset.text = text;
        el.textContent = text;
      }
      if (el.className !== cls) el.className = cls;
      el.style.transform = `translate(${s.x.toFixed(1)}px, ${s.y.toFixed(1)}px)`;
      el.style.opacity = opacity.toFixed(2);
      el.style.setProperty("--kb-c", color ?? "transparent");
      el.style.display = "";
    };
    if (g && pos) {
      const hoverSet = hover >= 0;
      if (options.labels && !focus && !hoverSet && !path.length) {
        for (const c of centroids) {
          const community = g.communities[c.rank];
          show(c.p, community.name, "kb-label kb-constellation", 0.9, css(communityColor(c.rank, light)), true);
        }
      }
      const e = eye();
      for (const a of agents) {
        if (!a.pos) continue;
        show([a.pos[0], a.pos[1] - 9, a.pos[2]], a.label, "kb-label kb-label-agent", 1, css(a.color));
      }
      for (const { i, kind } of labelItems) {
        const p = [pos[i * 3], pos[i * 3 + 1] - nodeSize[i] * 1.6, pos[i * 3 + 2]];
        const depth = Math.hypot(p[0] - e[0], p[1] - e[1], p[2] - e[2]);
        const important = kind === "selected" || kind === "path" || kind === "key";
        const fade = important ? 1 : clamp(1.6 - depth / (cam.dist * 1.4), 0.25, 1);
        show(p, g.labels[i], `kb-label kb-label-${kind}`, fade, css([baseColor[i * 3], baseColor[i * 3 + 1], baseColor[i * 3 + 2]]), !important);
      }
    }
    for (let j = k; j < labelPool.length; j++) if (labelPool[j].style.display !== "none") labelPool[j].style.display = "none";
  }

  // Effects ------------------------------------------------------------------------------------

  /** Lights nodes up with a ring; `delays` (seconds per node) staggers them. */
  function pulse(indices, color, delay = 0, delays = null) {
    const t = now();
    for (const i of indices) {
      if (i < 0 || i >= (g?.n ?? 0)) continue;
      pulseStart[i] = t + delay + (delays?.get(i) ?? 0);
      pulseColor.set(color, i * 3);
    }
    pulseDirty = styleDirty = true;
  }

  function shockwave(origin, depths, color) {
    const t = now();
    rings.push({ node: origin, start: t, dur: 1.6, max: 60, color });
    rings.push({ node: origin, start: t + 0.35, dur: 1.9, max: 120, color });
    pulse([...depths.keys()], color, 0, new Map([...depths].map(([i, d]) => [i, d * 0.4])));
  }

  function fxSprites(time) {
    const list = [];
    if (selected >= 0 && pos) {
      const s = nodeSize[selected] * 3.4 + Math.sin(time * 2.6) * 0.5;
      list.push({ p: [pos[selected * 3], pos[selected * 3 + 1], pos[selected * 3 + 2]], c: light ? [0.2, 0.2, 0.45, 0.9] : [1, 1, 1, 0.85], size: s, shape: 3 });
    }
    rings = rings.filter((r) => time - r.start < r.dur);
    for (const r of rings) {
      const k = (time - r.start) / r.dur;
      if (k < 0 || !pos) continue;
      list.push({ p: [pos[r.node * 3], pos[r.node * 3 + 1], pos[r.node * 3 + 2]], c: [...r.color, 1 - k], size: r.max * easeOut(k), shape: 3 });
    }
    for (const a of agents) {
      if (!a.pos) continue;
      list.push({ p: a.pos, c: [...a.color, 1], size: 6.5, shape: 4 });
    }
    return list;
  }

  function moveAgents(time, dt) {
    if (!pos) return;
    agents.forEach((a, k) => {
      let target;
      const phase = k * 2.1 + time * 0.6;
      if (a.nodes.length) {
        const b = boundsOf(a.nodes);
        target = [b.c[0] + Math.cos(phase) * 14, b.c[1] + 26 + Math.sin(phase * 1.3) * 4, b.c[2] + Math.sin(phase) * 14];
      } else {
        const ang = k * 1.7 + time * 0.05;
        target = [Math.cos(ang) * R * 1.08, R * 0.18, Math.sin(ang) * R * 1.08];
      }
      if (!a.pos) a.pos = target;
      const s = 1 - Math.exp(-dt * 2.2);
      a.pos = a.pos.map((v, j) => v + (target[j] - v) * s);
    });
  }

  // Drawing ------------------------------------------------------------------------------------

  function resize() {
    dpr = Math.min(2, window.devicePixelRatio || 1);
    width = Math.max(1, container.clientWidth);
    height = Math.max(1, container.clientHeight);
    const w = Math.round(width * dpr);
    const hh = Math.round(height * dpr);
    if (canvas.width !== w || canvas.height !== hh) {
      canvas.width = w;
      canvas.height = hh;
    }
  }

  function frame() {
    raf = requestAnimationFrame(frame);
    const t = performance.now();
    const dt = Math.min(0.1, (t - lastFrame) / 1000);
    lastFrame = t;
    if (paused || document.hidden || lost) return;
    resize();
    const time = now();

    if (cam.anim) {
      const k = Math.min(1, (t - cam.anim.start) / cam.anim.ms);
      const e = easeInOut(k);
      cam.target = cam.anim.from.map((v, j) => v + (cam.anim.to[j] - v) * e);
      cam.dist = Math.exp(Math.log(cam.anim.fromDist) + (Math.log(cam.anim.toDist) - Math.log(cam.anim.fromDist)) * e);
      if (k >= 1) cam.anim = null;
    } else if (options.motion && t - lastInteract > 6000) cam.yaw += dt * 0.035;
    updateMatrices();

    scanLevel += ((scanOn ? 1 : 0) - scanLevel) * Math.min(1, dt * 3);
    if (g && pos) {
      if (styleDirty) computeStyle();
      if (flowDirty) computeFlow();
      if (positionsDirty) fillPositions();
      if (positionsDirty || ++centroidTick % 30 === 0) computeCentroids();
      moveAgents(time, dt);
    }
    if (gl && res) drawGL(time);
    else if (fallback) draw2D();
    drawLabels();
    // Until the first positions arrive nothing was uploaded, so keep everything dirty.
    if (g && pos) positionsDirty = styleDirty = flowDirty = pulseDirty = false;
  }

  function drawGL(time) {
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.disable(gl.DEPTH_TEST);
    gl.enable(gl.BLEND);
    if (light) gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    else gl.blendFunc(gl.ONE, gl.ONE);

    if (!light) {
      const rot = view.slice();
      rot[12] = rot[13] = rot[14] = 0;
      gl.useProgram(res.star.p);
      gl.uniformMatrix4fv(res.star.u.uRot, false, rot);
      gl.uniformMatrix4fv(res.star.u.uProj, false, perspective(fov, width / height, 1, 10000));
      gl.uniform1f(res.star.u.uTime, time);
      gl.uniform1f(res.star.u.uDpr, dpr);
      gl.bindVertexArray(res.stars.vao);
      gl.drawArrays(gl.POINTS, 0, res.stars.count);
    }
    if (!g || !pos) return gl.bindVertexArray(null);

    if (positionsDirty) {
      upload(res.nodes.pos, pos);
      upload(res.edges.pos, edgePosBuf);
      upload(res.particles.ab, flow.ab);
    }
    if (styleDirty || pulseDirty) {
      upload(res.nodes.color, nodeColorBuf);
      upload(res.nodes.state, nodeStateBuf);
      upload(res.nodes.pulse, pulseColor);
      upload(res.edges.color, edgeColorBuf);
      upload(res.edges.t, edgeTBuf);
    }
    if (flowDirty) {
      upload(res.particles.color, flow.color);
      upload(res.particles.param, flow.param);
    }

    gl.useProgram(res.edge.p);
    gl.uniformMatrix4fv(res.edge.u.uView, false, view);
    gl.uniformMatrix4fv(res.edge.u.uProj, false, proj);
    gl.bindVertexArray(res.edges.vao);
    gl.drawArrays(gl.LINES, 0, g.m * 2);

    // Beams from agent satellites to the nodes they're on.
    const beamPos = [];
    const beamColor = [];
    for (const a of agents) {
      if (!a.pos) continue;
      for (const i of a.nodes) {
        beamPos.push(...a.pos, pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]);
        beamColor.push(...a.color, 0.7, ...a.color, 0.15);
      }
    }
    if (beamPos.length) {
      upload(res.beams.pos, new Float32Array(beamPos));
      upload(res.beams.color, new Float32Array(beamColor));
      upload(res.beams.t, new Float32Array((beamPos.length / 3) * 2));
      gl.bindVertexArray(res.beams.vao);
      gl.drawArrays(gl.LINES, 0, beamPos.length / 3);
    }

    if (flow.edges.length && (options.motion || path.length || focus)) {
      gl.useProgram(res.particle.p);
      gl.uniformMatrix4fv(res.particle.u.uView, false, view);
      gl.uniformMatrix4fv(res.particle.u.uProj, false, proj);
      gl.uniform1f(res.particle.u.uTime, time);
      gl.uniform1f(res.particle.u.uViewportH, canvas.height);
      gl.bindVertexArray(res.particles.vao);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, flow.edges.length);
    }

    gl.useProgram(res.node.p);
    gl.uniformMatrix4fv(res.node.u.uView, false, view);
    gl.uniformMatrix4fv(res.node.u.uProj, false, proj);
    gl.uniform1f(res.node.u.uTime, time);
    gl.uniform1f(res.node.u.uViewportH, canvas.height);
    gl.uniform1f(res.node.u.uLight, light ? 1 : 0);
    gl.uniform1f(res.node.u.uScan, scanLevel);
    gl.uniform1f(res.node.u.uScanR, ((time * 110) % (R * 1.5)) - R * 0.1);
    gl.bindVertexArray(res.nodes.vao);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, g.n);

    const fx = fxSprites(time);
    if (fx.length) {
      upload(res.fx.pos, new Float32Array(fx.flatMap((s) => s.p)));
      upload(res.fx.color, new Float32Array(fx.flatMap((s) => s.c)));
      upload(res.fx.state, new Float32Array(fx.flatMap((s) => [s.size, 2, 0, s.shape])));
      upload(res.fx.pulse, new Float32Array(fx.length * 3).fill(1));
      gl.uniform1f(res.node.u.uScan, 0);
      gl.bindVertexArray(res.fx.vao);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, fx.length);
    }
    gl.bindVertexArray(null);
  }

  /** The 2D fallback: lines and glowing dots, no particles. */
  function draw2D() {
    const c = fallback;
    c.setTransform(dpr, 0, 0, dpr, 0, 0);
    c.clearRect(0, 0, width, height);
    if (!g || !pos) return;
    const screen = new Array(g.n);
    for (let i = 0; i < g.n; i++) screen[i] = nodeStateBuf[i * 4] > 0 ? project(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]) : null;
    c.lineWidth = 1;
    for (let e = 0; e < g.m; e++) {
      const alpha = edgeColorBuf[e * 8 + 3];
      const a = screen[g.src[e]];
      const b = screen[g.dst[e]];
      if (!a || !b || alpha < 0.01) continue;
      c.strokeStyle = css([edgeColorBuf[e * 8], edgeColorBuf[e * 8 + 1], edgeColorBuf[e * 8 + 2]], Math.min(1, alpha * 2));
      c.beginPath();
      c.moveTo(a.x, a.y);
      c.lineTo(b.x, b.y);
      c.stroke();
    }
    c.globalCompositeOperation = light ? "source-over" : "lighter";
    for (let i = 0; i < g.n; i++) {
      const s = screen[i];
      if (!s) continue;
      const r = Math.max(1.5, (nodeStateBuf[i * 4] * (nodeStateBuf[i * 4 + 1] > 1.5 ? 1.5 : 1) * proj[5] * height * 0.5) / s.w);
      c.fillStyle = css([nodeColorBuf[i * 4], nodeColorBuf[i * 4 + 1], nodeColorBuf[i * 4 + 2]], nodeColorBuf[i * 4 + 3]);
      c.beginPath();
      c.arc(s.x, s.y, r * 0.55, 0, Math.PI * 2);
      c.fill();
    }
    c.globalCompositeOperation = "source-over";
  }

  // Picking and input --------------------------------------------------------------------------

  function pick(x, y) {
    if (!g || !pos) return -1;
    let best = -1;
    let bestScore = Infinity;
    for (let i = 0; i < g.n; i++) {
      const size = nodeStateBuf[i * 4];
      if (size <= 0) continue;
      const s = project(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]);
      if (!s) continue;
      const radius = Math.max(5, (size * proj[5] * height * 0.5) / s.w) + 3;
      const d = Math.hypot(s.x - x, s.y - y);
      if (d > radius) continue;
      // Prefer emphasized and nearer nodes.
      const score = d / radius + (nodeStateBuf[i * 4 + 1] < 1 ? 1 : 0) + s.w / (cam.dist * 50);
      if (score < bestScore) {
        bestScore = score;
        best = i;
      }
    }
    return best;
  }

  let drag = null;
  let hoverQueued = null;

  const local = (e) => {
    const r = canvas.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  };

  function onPointerDown(e) {
    canvas.setPointerCapture(e.pointerId);
    const [x, y] = local(e);
    drag = { x, y, sx: x, sy: y, button: e.button, pan: e.button === 2 || e.altKey, moved: false };
    cam.anim = null;
    lastInteract = performance.now();
  }

  function onPointerMove(e) {
    const [x, y] = local(e);
    if (drag) {
      const dx = x - drag.x;
      const dy = y - drag.y;
      drag.x = x;
      drag.y = y;
      if (Math.hypot(x - drag.sx, y - drag.sy) > 4) drag.moved = true;
      if (drag.pan) {
        const k = cam.dist * 0.0013;
        for (let j = 0; j < 3; j++) cam.target[j] += -view[j * 4] * dx * k + view[j * 4 + 1] * dy * k;
      } else {
        cam.yaw -= dx * 0.005;
        cam.pitch = clamp(cam.pitch + dy * 0.005, -1.45, 1.45);
      }
      lastInteract = performance.now();
      return;
    }
    if (!hoverQueued) {
      hoverQueued = requestAnimationFrame(() => {
        hoverQueued = null;
        const i = pick(x, y);
        if (i !== hover) {
          hover = i;
          styleDirty = flowDirty = true;
        }
        canvas.style.cursor = i >= 0 ? "pointer" : "grab";
        callbacks.onHover?.(i, x, y);
      });
    }
  }

  function onPointerUp(e) {
    const d = drag;
    drag = null;
    if (!d || d.moved) return;
    const [x, y] = local(e);
    const i = pick(x, y);
    callbacks.onClick?.(i, e);
  }

  function onWheel(e) {
    e.preventDefault();
    cam.anim = null;
    const scale = e.deltaMode === 1 ? 16 : 1;
    cam.dist = clamp(cam.dist * Math.exp(e.deltaY * scale * (e.ctrlKey ? 0.01 : 0.0012)), 15, 12000);
    lastInteract = performance.now();
  }

  function onDblClick(e) {
    const [x, y] = local(e);
    const i = pick(x, y);
    if (i >= 0) callbacks.onDblClick?.(i);
    else fit();
  }

  function onLeave() {
    if (hover >= 0) {
      hover = -1;
      styleDirty = flowDirty = true;
    }
    callbacks.onHover?.(-1, 0, 0);
  }

  canvas.addEventListener("pointerdown", onPointerDown);
  canvas.addEventListener("pointermove", onPointerMove);
  canvas.addEventListener("pointerup", onPointerUp);
  canvas.addEventListener("pointerleave", onLeave);
  canvas.addEventListener("wheel", onWheel, { passive: false });
  canvas.addEventListener("dblclick", onDblClick);
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());

  const themeObserver = new MutationObserver(() => {
    const next = document.documentElement.dataset.mode === "light";
    if (next !== light) {
      light = next;
      recolor();
    }
  });
  themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["data-mode"] });

  resize();
  updateMatrices();
  raf = requestAnimationFrame(frame);

  return {
    webgl: !!gl,
    setGraph,
    get graph() {
      return g;
    },
    get radius() {
      return R;
    },
    get settled() {
      return settled;
    },
    positions: () => pos,
    setVisible(mask) {
      visible = mask;
      styleDirty = flowDirty = true;
    },
    setPresence(mask) {
      presence = mask;
      styleDirty = flowDirty = true;
    },
    setConfidence(list) {
      confVisible = list;
      styleDirty = flowDirty = true;
    },
    setFocus(set, keyNodes) {
      focus = set && set.size ? set : null;
      keys = keyNodes ?? new Set();
      styleDirty = flowDirty = true;
    },
    setSelected(i) {
      selected = i;
      styleDirty = flowDirty = true;
    },
    setPath(nodes) {
      path = nodes;
      pathEdges = new Set();
      for (let k = 0; k + 1 < nodes.length; k++) {
        const e = edgeBetween(g, nodes[k], nodes[k + 1]);
        if (e >= 0) pathEdges.add(e);
      }
      styleDirty = flowDirty = true;
    },
    setOptions(next) {
      options = { ...options, ...next };
      styleDirty = flowDirty = true;
    },
    setAgents(list) {
      const old = new Map(agents.map((a) => [a.id, a]));
      agents = list.map((a) => ({ ...a, pos: old.get(a.id)?.pos ?? null }));
    },
    setScan(on) {
      scanOn = on;
    },
    setPaused(on) {
      paused = on;
    },
    pulse,
    shockwave,
    flyToNode(i, close = true) {
      if (!pos || i < 0) return;
      const p = [pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]];
      flyTo(p, close ? clamp(cam.dist * 0.5, 60, 260) : cam.dist);
    },
    flyToNodes: (indices) => fitIndices(indices, 1100),
    fit,
    screenOf(i) {
      if (!pos || i < 0) return null;
      return project(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]);
    },
    clear() {
      worker?.terminate();
      worker = null;
      g = null;
      pos = null;
      agents = [];
      rings = [];
      labelItems = [];
    },
    reheat(alpha = 0.3) {
      worker?.postMessage({ type: "reheat", alpha });
    },
    destroy() {
      cancelAnimationFrame(raf);
      worker?.terminate();
      themeObserver.disconnect();
      canvas.remove();
      labelLayer.remove();
    },
  };
}

// Page -------------------------------------------------------------------------------------------

/** Replaces an element's children, skipping null, false and undefined like h() does. */
function fill(el, ...children) {
  el.replaceChildren(...children.flat().filter((c) => c !== null && c !== undefined && c !== false));
}

const isLight = () => document.documentElement.dataset.mode === "light";
const nodeCss = (g, i, alpha = 1) => css(communityColor(g.commRank[i], isLight()), alpha);
const fileRef = (g, i) => (g.files[i] ? `${g.files[i]}${g.locs[i] ? `:${g.locs[i].replace(/^L/, "")}` : ""}` : "");
const typeLabel = (t) => TYPE_LABELS[t] ?? t.charAt(0).toUpperCase() + t.slice(1);
const typeIcon = (t) => (t === "code" ? "terminal" : t === "document" || t === "paper" ? "file" : t === "rationale" ? "book" : "sparkles");
const typeGlyph = (t) => (t === "document" || t === "paper" ? "◎" : t === "concept" || t === "rationale" ? "✦" : "●");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function communityNodes(g, id) {
  const out = [];
  for (let i = 0; i < g.n; i++) if (g.comm[i] === id) out.push(i);
  return out;
}

/** Ranks nodes for a search: exact, prefix, word start, substring, file path, then subsequence. */
function searchNodes(g, text, limit = 12) {
  const query = text.trim().toLowerCase();
  if (!query) return [];
  const results = [];
  for (let i = 0; i < g.n; i++) {
    const label = g.labels[i].toLowerCase();
    let score = 0;
    if (label === query) score = 100;
    else if (label.startsWith(query)) score = 80;
    else if (new RegExp(`[^a-z0-9]${query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`).test(label)) score = 60;
    else if (label.includes(query)) score = 50;
    else if (g.files[i].toLowerCase().includes(query)) score = 30;
    else {
      let j = 0;
      for (const ch of label) if (ch === query[j]) j++;
      if (j === query.length) score = 10;
    }
    if (score) results.push([i, score + Math.min(12, g.degree[i] / 4)]);
  }
  return results
    .sort((a, b) => b[1] - a[1])
    .slice(0, limit)
    .map(([i]) => i);
}

function countUp(el, to) {
  const start = performance.now();
  const step = (t) => {
    const k = Math.min(1, (t - start) / 900);
    el.textContent = Math.round(to * easeOut(k)).toLocaleString();
    if (k < 1 && el.isConnected) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

function mountPage(el, store, api) {
  const style = h("style", {}, CSS);
  const shell = h("div", { class: "kb", tabindex: "-1" });
  el.append(style, shell);

  const ui = {
    workspaceId: null,
    root: null,
    version: -1,
    tab: "galaxy",
    mode: "explore",
    selected: -1,
    pathFrom: -1,
    path: [],
    impact: null,
    impactDepth: 2,
    community: null,
    answer: null,
    hiddenTypes: new Set(),
    conf: [true, true, true],
    minDegree: 0,
    legendOpen: true,
    legendAuto: true,
    menu: null,
    searchIndex: 0,
    timeline: { index: -1, playing: false },
    presence: null,
    counted: "",
    hasKey: false,
    keyDraft: "",
  };
  const agentState = new Map();

  const nebula = h("div", { class: "kb-nebula" });
  const stage = h("div", { class: "kb-stage" });
  const vignette = h("div", { class: "kb-vignette" });
  const top = h("header", { class: "kb-top" });
  const banner = h("div", { class: "kb-banner" });
  const legend = h("aside", { class: "kb-legend kb-glass" });
  const legendFilter = h("input", { class: "kb-legend-filter", placeholder: "Filter communities", spellcheck: false, oninput: () => renderLegendList() });
  const legendList = h("div", { class: "kb-legend-list" });
  const inspector = h("aside", { class: "kb-inspector kb-glass" });
  const feed = h("div", { class: "kb-feed" });
  const answer = h("div", { class: "kb-answer kb-glass" });
  const askInput = h("input", { class: "kb-ask-input", placeholder: "Ask the knowledge graph anything…", spellcheck: false });
  const askForm = h(
    "form",
    {
      class: "kb-ask",
      onsubmit: (e) => {
        e.preventDefault();
        void ask(askInput.value);
      },
    },
    h("span", { class: "kb-ask-beam" }),
    icon("sparkles", "kb-i kb-ask-icon"),
    askInput,
    h("button", { class: "kb-ask-send", type: "submit", title: "Ask (Enter)" }, icon("send")),
  );
  const dock = h("div", { class: "kb-dock kb-glass" });
  const hoverCard = h("div", { class: "kb-hovercard kb-glass" });
  hoverCard.hidden = true;
  const searchInput = h("input", { class: "kb-search-input", placeholder: "Search nodes and files…", spellcheck: false });
  const searchResults = h("div", { class: "kb-search-results" });
  const search = h("div", { class: "kb-search kb-glass" }, h("div", { class: "kb-search-row" }, icon("search"), searchInput, h("kbd", {}, "esc")), searchResults);
  search.hidden = true;
  const overlay = h("div", { class: "kb-overlay" });
  const menus = h("div", { class: "kb-menus" });
  shell.append(nebula, stage, vignette, overlay, banner, top, legend, inspector, feed, answer, askForm, dock, hoverCard, search, menus);

  const galaxy = createGalaxy(stage, {
    onHover: showHover,
    onClick: click,
    onDblClick(i) {
      select(i, false);
      galaxy.flyToNode(i, true);
    },
  });
  galaxy.setOptions({ motion: store.settings.motion, flow: store.settings.flow, labels: store.settings.labels });

  const state = () => api.state();
  function workspace() {
    const s = state();
    return s.workspaces.find((w) => w.id === (ui.workspaceId ?? s.activeWorkspaceId)) ?? s.workspaces[0];
  }
  const current = () => {
    const ws = workspace();
    return ws ? store.kb(ws.rootDir) : null;
  };
  const graph = () => galaxy.graph;
  const accent = () => cssColor("--color-accent", [0.43, 0.49, 1]);
  const danger = () => cssColor("--color-danger", [0.95, 0.43, 0.43]);

  function spawnAgent(prompt) {
    const ws = workspace();
    if (state().activeWorkspaceId !== ws.id) api.execute({ type: "workspace.switch", target: { id: ws.id } });
    api.execute({ type: "pane.spawn", provider: "default", prompt });
    api.execute({ type: "ui.view", view: "workspace" });
  }

  async function copy(text, what = "Copied") {
    try {
      await navigator.clipboard.writeText(text);
      api.toast(what, "success");
    } catch {
      api.toast("Couldn't copy to the clipboard", "error");
    }
  }

  // Graph sync ---------------------------------------------------------------------------------

  function saveLayout() {
    const g = graph();
    const p = galaxy.positions();
    if (!g || !p || !ui.root) return;
    store.kb(ui.root).layout = { index: g.index, pos: p.slice() };
  }

  function cachedLayout(k) {
    if (!k.layout) return null;
    const g = k.graph;
    const out = new Float32Array(g.n * 3).fill(NaN);
    let hits = 0;
    for (let i = 0; i < g.n; i++) {
      const j = k.layout.index.get(g.ids[i]);
      if (j === undefined || j * 3 + 2 >= k.layout.pos.length) continue;
      out[i * 3] = k.layout.pos[j * 3];
      out[i * 3 + 1] = k.layout.pos[j * 3 + 1];
      out[i * 3 + 2] = k.layout.pos[j * 3 + 2];
      hits++;
    }
    return hits ? out : null;
  }

  function resetView() {
    ui.selected = -1;
    ui.pathFrom = -1;
    ui.path = [];
    ui.impact = null;
    ui.community = null;
    ui.answer = null;
    ui.timeline = { index: -1, playing: false };
    ui.presence = null;
  }

  function setNebula(g) {
    const light = isLight();
    [0, 1, 2].forEach((rank) => shell.style.setProperty(`--kb-n${rank + 1}`, css(communityColor(rank, light), light ? 0.16 : 0.2)));
    if (!g) return;
  }

  /** Shows the current workspace's graph, keeping the layout and selection across reloads. */
  function sync() {
    const k = current();
    if (!k) return;
    if (k.root !== ui.root) {
      saveLayout();
      galaxy.clear();
      ui.root = k.root;
      ui.version = -1;
      resetView();
      agentState.clear();
      if (k.status === "idle") void store.load(k.root);
    }
    if (k.graph && k.version !== ui.version) {
      const reload = ui.version !== -1;
      const old = graph();
      const selectedId = old && ui.selected >= 0 ? old.ids[ui.selected] : null;
      if (reload) saveLayout();
      ui.version = k.version;
      const fresh = reload && k.previousIds ? new Set(k.graph.ids.filter((id) => !k.previousIds.has(id))) : null;
      galaxy.setGraph(k.graph, cachedLayout(k), fresh);
      setNebula(k.graph);
      const keep = selectedId ? (k.graph.index.get(selectedId) ?? -1) : -1;
      resetView();
      ui.selected = keep;
      if (ui.minDegree > 0 && !reload) ui.minDegree = 0;
      applyFilters();
      applyFocus();
    }
    if (!k.graph && graph()) galaxy.clear();
    galaxy.setScan(!!k.building);
  }

  // Focus, filters and modes -------------------------------------------------------------------

  function applyFilters() {
    const g = graph();
    if (!g) return;
    const mask = new Uint8Array(g.n);
    for (let i = 0; i < g.n; i++) mask[i] = !ui.hiddenTypes.has(g.types[i]) && g.degree[i] >= ui.minDegree ? 1 : 0;
    galaxy.setVisible(mask);
    galaxy.setConfidence(ui.conf);
  }

  function applyFocus() {
    const g = graph();
    if (!g) return;
    let set = null;
    let keys = new Set();
    if (ui.path.length) {
      set = new Set(ui.path);
      keys = new Set([ui.path[0], ui.path[ui.path.length - 1]]);
    } else if (ui.impact) {
      set = new Set(ui.impact.depths.keys());
      keys = new Set([ui.impact.origin]);
    } else if (ui.selected >= 0) {
      set = new Set([ui.selected, ...neighbors(g, ui.selected)]);
      keys = new Set([ui.selected]);
    } else if (ui.answer?.nodes?.length) {
      set = new Set(ui.answer.nodes);
      keys = new Set(ui.answer.nodes.slice(0, 6));
    } else if (ui.community !== null) {
      set = new Set(communityNodes(g, ui.community));
    }
    if (ui.pathFrom >= 0) keys.add(ui.pathFrom);
    galaxy.setFocus(set, keys);
    galaxy.setSelected(ui.impact ? ui.impact.origin : ui.selected >= 0 ? ui.selected : ui.pathFrom);
    galaxy.setPath(ui.path);
  }

  function select(i, fly = true) {
    ui.selected = i;
    ui.path = [];
    ui.impact = null;
    ui.pathFrom = -1;
    applyFocus();
    renderInspector();
    if (fly && i >= 0) galaxy.flyToNode(i, true);
  }

  function clearAll() {
    ui.selected = -1;
    ui.path = [];
    ui.impact = null;
    ui.pathFrom = -1;
    ui.community = null;
    ui.answer = null;
    applyFocus();
    renderInspector();
    renderAnswer();
    renderLegendList();
  }

  function setMode(mode) {
    ui.mode = mode;
    ui.pathFrom = -1;
    if (mode !== "path") ui.path = [];
    if (mode !== "impact") ui.impact = null;
    if (mode === "path" && ui.selected >= 0) ui.pathFrom = ui.selected;
    applyFocus();
    renderDock();
    renderInspector();
  }

  function click(i, e) {
    const g = graph();
    if (!g) return;
    ui.menu = null;
    renderMenus();
    if (i < 0) {
      if (ui.mode === "explore" && !e.shiftKey) clearAll();
      return;
    }
    if (ui.mode === "path" || e.shiftKey) return pathClick(i);
    if (ui.mode === "impact") return runImpact(i);
    select(i);
  }

  function pathClick(i) {
    const g = graph();
    if (ui.pathFrom < 0 || ui.path.length) {
      ui.pathFrom = i;
      ui.path = [];
      ui.selected = -1;
      ui.impact = null;
      applyFocus();
      renderInspector();
      return;
    }
    const route = shortestPath(g, ui.pathFrom, i);
    if (!route.length) {
      api.toast(`“${g.labels[ui.pathFrom]}” and “${g.labels[i]}” aren't connected`, "error");
      return;
    }
    ui.path = route;
    ui.pathFrom = -1;
    ui.selected = -1;
    applyFocus();
    galaxy.flyToNodes(route);
    renderInspector();
  }

  function runImpact(i) {
    const g = graph();
    const depths = affected(g, i, ui.impactDepth);
    ui.impact = { origin: i, depths };
    ui.selected = -1;
    ui.path = [];
    applyFocus();
    galaxy.shockwave(i, depths, danger());
    if (depths.size > 1) galaxy.flyToNodes([...depths.keys()]);
    else galaxy.flyToNode(i, true);
    renderInspector();
  }

  function isolate(id) {
    const g = graph();
    ui.community = ui.community === id ? null : id;
    ui.selected = -1;
    ui.path = [];
    ui.impact = null;
    ui.answer = null;
    applyFocus();
    renderInspector();
    renderAnswer();
    renderLegendList();
    if (ui.community !== null) galaxy.flyToNodes(communityNodes(g, id));
    else galaxy.fit();
  }

  // Ask ----------------------------------------------------------------------------------------

  async function ask(question) {
    question = question.trim();
    const k = current();
    if (!question || !k?.graph) return;
    ui.answer = { question, busy: true, text: "", nodes: [] };
    renderAnswer();
    try {
      const r = await store.exec(k.root, `${store.bin} query ${q(question)} --budget 2000`, 90);
      if (r.code !== 0) throw new Error(lastLines(r.output) || "graphify query failed");
      const nodes = nodesInOutput(k.graph, r.output);
      ui.answer = { question, busy: false, text: r.output.trim(), nodes };
      ui.selected = -1;
      ui.path = [];
      ui.impact = null;
      ui.community = null;
      applyFocus();
      galaxy.pulse(nodes, accent(), 0, new Map(nodes.map((i, n) => [i, n * 0.05])));
      if (nodes.length) galaxy.flyToNodes(nodes);
    } catch (err) {
      ui.answer = { question, busy: false, text: "", nodes: [], error: String(err?.message ?? err) };
    }
    renderAnswer();
    renderInspector();
  }

  // Timeline -----------------------------------------------------------------------------------

  async function setTimeline(index) {
    const k = current();
    const g = graph();
    if (!k || !g) return;
    ui.timeline.index = index;
    const before = ui.presence;
    if (index < 0 || index >= k.snapshots.length) {
      ui.presence = null;
      galaxy.setPresence(null);
      if (before) galaxy.pulse([...Array(g.n).keys()].filter((i) => !before[i]), [0.55, 1, 0.8]);
      updateTimelineLabel();
      return;
    }
    const ids = await store.snapshotIds(k.root, k.snapshots[index]);
    if (ui.timeline.index !== index || graph() !== g) return;
    const mask = new Uint8Array(g.n);
    const added = [];
    for (let i = 0; i < g.n; i++) {
      mask[i] = ids.has(g.ids[i]) ? 1 : 0;
      if (mask[i] && before && !before[i]) added.push(i);
    }
    ui.presence = mask;
    galaxy.setPresence(mask);
    if (added.length) galaxy.pulse(added, [0.55, 1, 0.8]);
    updateTimelineLabel();
  }

  async function replay() {
    const k = current();
    if (!k?.snapshots.length) return;
    if (ui.timeline.playing) {
      ui.timeline.playing = false;
      renderDock();
      return;
    }
    ui.timeline.playing = true;
    renderDock();
    const count = k.snapshots.length;
    const steps = count <= 14 ? [...Array(count).keys()] : [...Array(14).keys()].map((s) => Math.round((s * (count - 1)) / 13));
    ui.presence = null;
    for (const s of [...new Set(steps)]) {
      if (!ui.timeline.playing || !shell.isConnected) return;
      await setTimeline(s);
      await sleep(1100);
    }
    if (ui.timeline.playing) await setTimeline(-1);
    ui.timeline.playing = false;
    renderDock();
  }

  let timelineLabel = null;
  let timelineSlider = null;
  function updateTimelineLabel() {
    const k = current();
    if (!timelineLabel || !k) return;
    const i = ui.timeline.index;
    timelineLabel.textContent = i < 0 ? "Now" : k.snapshots[i].replace("_", " #");
    if (timelineSlider) timelineSlider.value = String(i < 0 ? k.snapshots.length : i);
  }

  // Agents -------------------------------------------------------------------------------------

  function pushAgents() {
    galaxy.setAgents(
      [...agentState.values()].map((a) => ({
        id: a.id,
        color: a.color,
        label: a.label,
        nodes: a.nodes,
      })),
    );
  }

  /** Follows what each agent of the workspace is working on, from the files its terminal shows. */
  async function scanAgents() {
    const g = graph();
    const ws = workspace();
    if (!g || !ws || document.hidden || ui.tab !== "galaxy") return;
    const panes = ws.panes.filter((p) => p.provider !== "shell");
    const alive = new Set(panes.map((p) => p.id));
    for (const id of agentState.keys()) if (!alive.has(id)) agentState.delete(id);
    for (const pane of panes) {
      let a = agentState.get(pane.id);
      if (!a) {
        const who = api.describePane(pane.id);
        a = { id: pane.id, color: providerColor(pane.provider), label: who?.label ?? pane.provider, nodes: [], read: false };
        agentState.set(pane.id, a);
      }
      if (pane.status !== "working" && a.read) continue;
      a.read = true;
      const text = await api.readPane(pane.id, 80);
      if (!text || graph() !== g) continue;
      const found = [];
      for (const file of filesInText(text)) {
        const i = resolveFile(g, file, ws.rootDir);
        if (i < 0) continue;
        const at = found.indexOf(i);
        if (at >= 0) found.splice(at, 1);
        found.push(i);
      }
      const nodes = found.slice(-5);
      const added = nodes.filter((i) => !a.nodes.includes(i));
      a.nodes = nodes;
      if (added.length) galaxy.pulse(added, a.color);
    }
    pushAgents();
  }

  function onActivity(entry) {
    const g = graph();
    if (!g || entry.root !== ui.root) return;
    const nodes = nodesInOutput(g, entry.output).slice(0, 24);
    const color = providerColor(entry.provider);
    galaxy.pulse(nodes, color, 0, new Map(nodes.map((i, n) => [i, n * 0.04])));
    let a = agentState.get(entry.paneId);
    if (!a) {
      a = { id: entry.paneId, color, label: entry.label, nodes: [], read: true };
      agentState.set(entry.paneId, a);
    }
    if (nodes.length) a.nodes = nodes.slice(0, 6);
    pushAgents();
    renderFeed();
  }

  // Rendering ----------------------------------------------------------------------------------

  function renderAll() {
    sync();
    renderTop();
    renderBanner();
    renderOverlay();
    renderLegend();
    renderInspector();
    renderAnswer();
    renderDock();
    renderFeed();
    renderMenus();
    askForm.hidden = !graph() || ui.tab !== "galaxy";
  }

  function renderTop() {
    const k = current();
    const g = k?.graph;
    const s = state();
    const ws = workspace();
    const select = h(
      "select",
      {
        class: "kb-ws-select",
        title: "Project",
        onchange: (e) => {
          const id = e.target.value;
          ui.workspaceId = id === s.activeWorkspaceId ? null : id;
          renderAll();
        },
      },
      s.workspaces.map((w) => h("option", { value: w.id, selected: w.id === ws?.id }, w.name)),
    );
    const stat = (n, label) => {
      const b = h("b", {}, n.toLocaleString());
      if (ui.counted !== `${ui.root}:${k?.version}`) countUp(b, n);
      return h("span", { class: "kb-stat" }, b, h("span", {}, label));
    };
    const stale = g && k.head && g.commit && !k.head.startsWith(g.commit) && !g.commit.startsWith(k.head);
    const building = k?.building;
    const tab = (id, label, ic) =>
      h(
        "button",
        {
          class: `kb-tab${ui.tab === id ? " kb-on" : ""}`,
          onclick: () => {
            ui.tab = id;
            renderAll();
          },
        },
        icon(ic),
        h("span", { class: "kb-tab-label" }, label),
      );

    fill(top, 
      h("div", { class: "kb-brand kb-glass" }, h("span", { class: "kb-orb" }), h("span", { class: "kb-title" }, "Knowledge"), h("span", { class: "kb-brand-sep" }), select),
      g &&
        h(
          "div",
          { class: "kb-stats kb-glass" },
          stat(g.n, "nodes"),
          stat(g.m, "links"),
          stat(g.communities.length, "communities"),
        ),
      g &&
        !building &&
        h(
          "button",
          {
            class: `kb-chip kb-glass${stale ? " kb-warn" : ""}`,
            title: stale ? `Built from ${g.commit.slice(0, 8)}; the project is at ${k.head.slice(0, 8)}. Click to update.` : `Built from ${g.commit?.slice(0, 8) ?? "an unknown commit"}`,
            onclick: () => stale && void store.build(k.root, "quick"),
          },
          icon(stale ? "alert" : "clock"),
          h("span", { class: "kb-chip-text" }, stale ? "Behind HEAD · update" : `Built ${ago(k.builtAt)}`),
        ),
      building &&
        h(
          "div",
          { class: "kb-chip kb-glass kb-building" },
          h("span", { class: "kb-spinner" }),
          h("span", { class: "kb-chip-text" }, building.mode === "agent" ? "An agent is mapping the project" : building.mode === "headless" ? "Deep build running" : "Mapping code"),
          h("span", { class: "kb-elapsed" }, elapsed(Date.now() - building.startedAt)),
          building.mode === "agent" && h("button", { class: "kb-link", onclick: () => store.cancelBuild(k.root) }, "Stop waiting"),
        ),
      h("span", { class: "kb-grow" }),
      g &&
        h(
          "button",
          { class: "kb-btn kb-glass kb-icon-only", title: "Search (/ or ⌘F)", onclick: () => openSearch() },
          icon("search"),
        ),
      g && h("div", { class: "kb-tabs kb-glass" }, tab("galaxy", "Galaxy", "orbit"), tab("insights", "Insights", "grid")),
      k &&
        store.graphify.state !== "missing" &&
        h(
          "div",
          { class: "kb-split" },
          h(
            "button",
            {
              class: "kb-btn kb-primary",
              disabled: !!building,
              title: "Re-read the code (graphify update, no AI)",
              onclick: () => void store.build(k.root, "quick"),
            },
            icon("refresh"),
            h("span", { class: "kb-btn-label" }, g ? "Update" : "Map code"),
          ),
          h(
            "button",
            {
              class: "kb-btn kb-primary kb-split-more",
              "data-menu": "build",
              title: "More ways to build",
              onclick: () => toggleMenu("build"),
            },
            icon("chevron"),
          ),
        ),
      h(
        "button",
        { class: `kb-btn kb-glass kb-icon-only${ui.menu === "settings" ? " kb-on" : ""}`, "data-menu": "settings", title: "Settings", onclick: () => toggleMenu("settings") },
        icon("sliders"),
      ),
    );
    if (g) ui.counted = `${ui.root}:${k.version}`;
  }

  function renderBanner() {
    const k = current();
    const parts = [];
    if (k?.graph && k.error) {
      parts.push(
        h(
          "div",
          { class: "kb-alert kb-glass" },
          icon("alert"),
          h("span", { class: "kb-grow" }, k.error),
          k.needsForce && h("button", { class: "kb-btn", onclick: () => void store.build(k.root, "quick", { force: true }) }, "Force update"),
          h(
            "button",
            {
              class: "kb-icon-btn",
              onclick: () => {
                k.error = "";
                renderBanner();
              },
            },
            icon("x"),
          ),
        ),
      );
    }
    if (k?.graph && k.needsForce && !k.error) {
      parts.push(
        h(
          "div",
          { class: "kb-alert kb-glass kb-alert-info" },
          icon("alert"),
          h("span", { class: "kb-grow" }, "graphify kept the old graph because the new one has fewer nodes (code was deleted?)."),
          h("button", { class: "kb-btn", onclick: () => void store.build(k.root, "quick", { force: true }) }, "Use the new graph"),
        ),
      );
    }
    if (ui.mode !== "explore" && graph() && ui.tab === "galaxy") {
      const text =
        ui.mode === "path"
          ? ui.pathFrom >= 0
            ? `From “${graph().labels[ui.pathFrom]}”: now click where the path should end`
            : ui.path.length
              ? "Click another node to start a new path"
              : "Click the node where the path starts (or shift-click in Explore)"
          : "Click a node to see everything that depends on it";
      parts.push(h("div", { class: "kb-hint kb-glass" }, icon(ui.mode === "path" ? "route" : "zap"), text));
    }
    fill(banner, ...parts);
  }

  function renderLegend() {
    const g = graph();
    legend.hidden = !g || ui.tab !== "galaxy";
    if (legend.hidden) return;
    if (!ui.legendOpen) {
      legend.className = "kb-legend kb-legend-closed";
      fill(legend, 
        h(
          "button",
          {
            class: "kb-btn kb-glass",
            onclick: () => {
              ui.legendOpen = true;
              ui.legendAuto = false;
              renderLegend();
            },
          },
          icon("layers"),
          plural(g.communities.length, "community").replace("communitys", "communities"),
        ),
      );
      return;
    }
    legend.className = "kb-legend kb-glass";
    fill(legend, 
      h(
        "div",
        { class: "kb-panel-head" },
        h("span", { class: "kb-eyebrow" }, "Communities"),
        h("span", { class: "kb-count" }, g.communities.length.toLocaleString()),
        h("span", { class: "kb-grow" }),
        h(
          "button",
          {
            class: "kb-icon-btn",
            title: "Hide",
            onclick: () => {
              ui.legendOpen = false;
              ui.legendAuto = false;
              renderLegend();
            },
          },
          icon("x"),
        ),
      ),
      legendFilter,
      legendList,
    );
    renderLegendList();
  }

  function renderLegendList() {
    const g = graph();
    if (!g || legend.hidden || !ui.legendOpen) return;
    const filter = legendFilter.value.trim().toLowerCase();
    const list = g.communities.filter((c) => !filter || c.name.toLowerCase().includes(filter)).slice(0, 150);
    const max = g.communities[0]?.count ?? 1;
    const light = isLight();
    fill(legendList, 
      ...list.map((c) =>
        h(
          "button",
          {
            class: `kb-comm${ui.community === c.id ? " kb-on" : ""}`,
            style: `--c:${css(communityColor(c.rank, light))};--w:${((c.count / max) * 100).toFixed(1)}%`,
            title: c.name,
            onclick: () => isolate(c.id),
            onmouseenter: () => {
              if (ui.selected < 0 && !ui.path.length && !ui.impact) galaxy.setFocus(new Set(communityNodes(g, c.id)), new Set());
            },
            onmouseleave: () => applyFocus(),
          },
          h("span", { class: "kb-swatch" }),
          h("span", { class: "kb-comm-name" }, c.name),
          h("span", { class: "kb-comm-n" }, c.count.toLocaleString()),
        ),
      ),
      list.length ? null : h("div", { class: "kb-empty-small" }, "No community matches."),
    );
  }

  function nodeRow(g, i, extra) {
    return h(
      "button",
      { class: "kb-node-row", onclick: () => select(i), title: fileRef(g, i) },
      h("span", { class: "kb-dot", style: `--c:${nodeCss(g, i)}` }),
      h("span", { class: "kb-node-name" }, g.labels[i]),
      extra,
    );
  }

  function renderInspector() {
    const g = graph();
    const show = g && ui.tab === "galaxy" && (ui.selected >= 0 || ui.path.length || ui.impact);
    inspector.hidden = !show;
    if (!show) return;
    const closeBtn = h("button", { class: "kb-icon-btn", title: "Close (Esc)", onclick: clearAll }, icon("x"));

    if (ui.path.length) {
      const from = ui.path[0];
      const to = ui.path[ui.path.length - 1];
      const steps = [];
      ui.path.forEach((i, k) => {
        const file = baseName(g.files[i] || "");
        steps.push(h("div", { class: "kb-step" }, nodeRow(g, i, file !== g.labels[i] ? h("span", { class: "kb-node-file" }, file) : null)));
        if (k + 1 < ui.path.length) {
          const e = edgeBetween(g, i, ui.path[k + 1]);
          const forward = g.src[e] === i;
          steps.push(h("div", { class: "kb-step-rel" }, icon(forward ? "arrowRight" : "arrowLeft"), g.relations[g.rel[e]], g.conf[e] ? h("span", { class: "kb-conf" }, CONFIDENCE[g.conf[e]].toLowerCase()) : null));
        }
      });
      fill(inspector, 
        h("div", { class: "kb-panel-head" }, icon("route"), h("span", { class: "kb-eyebrow" }, "Path"), h("span", { class: "kb-count" }, plural(ui.path.length - 1, "hop")), h("span", { class: "kb-grow" }), closeBtn),
        h("div", { class: "kb-insp-title" }, g.labels[from], h("span", { class: "kb-arrow" }, " → "), g.labels[to]),
        h("div", { class: "kb-steps" }, steps),
        h(
          "div",
          { class: "kb-actions" },
          h(
            "button",
            {
              class: "kb-btn kb-primary",
              onclick: () =>
                spawnAgent(
                  `In this project, explain how ${g.labels[from]} (${fileRef(g, from)}) connects to ${g.labels[to]} (${fileRef(g, to)}). The knowledge graph links them through: ${ui.path.map((i) => g.labels[i]).join(" → ")}. Use the kb_path and kb_explain tools from winfer's knowledge base, then read the code.`,
                ),
            },
            icon("bot"),
            "Ask an agent",
          ),
          h("button", { class: "kb-btn", onclick: () => copy(ui.path.map((i) => `${g.labels[i]}${g.files[i] ? ` (${fileRef(g, i)})` : ""}`).join("\n→ "), "Path copied") }, icon("copy"), "Copy"),
        ),
      );
      return;
    }

    if (ui.impact) {
      const { origin, depths } = ui.impact;
      const byDepth = new Map();
      for (const [i, d] of depths) if (d > 0) byDepth.set(d, [...(byDepth.get(d) ?? []), i]);
      const total = depths.size - 1;
      fill(inspector, 
        h("div", { class: "kb-panel-head" }, icon("zap"), h("span", { class: "kb-eyebrow" }, "Impact"), h("span", { class: "kb-count kb-count-danger" }, plural(total, "node")), h("span", { class: "kb-grow" }), closeBtn),
        h("div", { class: "kb-insp-title" }, g.labels[origin]),
        g.files[origin] && h("div", { class: "kb-insp-file" }, icon("file"), fileRef(g, origin)),
        h(
          "div",
          { class: "kb-depths" },
          h("span", { class: "kb-subtle" }, "Depth"),
          [1, 2, 3, 4].map((d) =>
            h(
              "button",
              {
                class: `kb-seg kb-seg-small${ui.impactDepth === d ? " kb-on" : ""}`,
                onclick: () => {
                  ui.impactDepth = d;
                  runImpact(origin);
                },
              },
              String(d),
            ),
          ),
        ),
        total === 0 && h("div", { class: "kb-empty-small" }, "Nothing calls, imports or extends this node. Changing it is unlikely to break other code."),
        h(
          "div",
          { class: "kb-groups" },
          [...byDepth.entries()].map(([d, list]) =>
            h(
              "div",
              { class: "kb-group" },
              h("div", { class: "kb-group-head" }, d === 1 ? "Direct" : `Depth ${d}`, h("span", { class: "kb-count" }, list.length)),
              list.slice(0, 60).map((i) => nodeRow(g, i, h("span", { class: "kb-node-file" }, baseName(g.files[i] || "")))),
              list.length > 60 && h("div", { class: "kb-more" }, `+${list.length - 60} more`),
            ),
          ),
        ),
        h(
          "div",
          { class: "kb-actions" },
          h(
            "button",
            {
              class: "kb-btn kb-primary",
              onclick: () =>
                spawnAgent(
                  `I want to change ${g.labels[origin]} (${fileRef(g, origin)}). The knowledge graph says ${plural(total, "node")} depend on it: ${[...depths.keys()]
                    .filter((i) => i !== origin)
                    .slice(0, 40)
                    .map((i) => `${g.labels[i]} (${fileRef(g, i)})`)
                    .join(", ")}. Review what a change would affect and what to watch out for. Use kb_affected and kb_explain from winfer's knowledge base.`,
                ),
            },
            icon("bot"),
            "Review with an agent",
          ),
          h("button", { class: "kb-btn", onclick: () => copy([...depths.keys()].map((i) => `${g.labels[i]}\t${fileRef(g, i)}`).join("\n"), "Impact list copied") }, icon("copy"), "Copy"),
        ),
      );
      return;
    }

    const i = ui.selected;
    const c = g.communityById.get(g.comm[i]);
    const groups = new Map();
    let incoming = 0;
    for (let k = g.adjStart[i]; k < g.adjStart[i + 1]; k++) {
      const e = g.adjEdge[k];
      const out = g.src[e] === i;
      if (!out) incoming++;
      const key = `${g.relations[g.rel[e]]}${out ? " →" : " ←"}`;
      const list = groups.get(key) ?? [];
      list.push([otherEnd(g, e, i), e]);
      groups.set(key, list);
    }
    const hyper = g.hyperedges.filter((x) => x.nodes.includes(i));
    fill(inspector, 
      h(
        "div",
        { class: "kb-panel-head" },
        h("span", { class: "kb-type" }, icon(typeIcon(g.types[i])), typeLabel(g.types[i])),
        h(
          "button",
          { class: "kb-comm-chip", style: `--c:${nodeCss(g, i)}`, title: "Show this community", onclick: () => isolate(c.id) },
          h("span", { class: "kb-swatch" }),
          c.name,
        ),
        h("span", { class: "kb-grow" }),
        closeBtn,
      ),
      h("div", { class: `kb-insp-title${g.types[i] === "code" ? " kb-mono" : ""}` }, g.labels[i]),
      g.files[i] && h("button", { class: "kb-insp-file", title: "Copy file and line", onclick: () => copy(fileRef(g, i), "Reference copied") }, icon("file"), fileRef(g, i), icon("copy", "kb-i kb-i-hover")),
      h(
        "div",
        { class: "kb-metrics" },
        h("div", {}, h("b", {}, g.degree[i]), h("span", {}, "links")),
        h("div", {}, h("b", {}, incoming), h("span", {}, "incoming")),
        h("div", {}, h("b", {}, g.degree[i] - incoming), h("span", {}, "outgoing")),
        h("div", {}, h("b", {}, c.count), h("span", {}, "in community")),
      ),
      h(
        "div",
        { class: "kb-actions" },
        h(
          "button",
          {
            class: "kb-btn kb-primary",
            onclick: () =>
              spawnAgent(
                `Explain ${g.labels[i]}${g.files[i] ? ` (${fileRef(g, i)})` : ""} in this project: what it does, who uses it and why. Start with the kb_explain tool from winfer's knowledge base, then read the code.`,
              ),
          },
          icon("bot"),
          "Ask an agent",
        ),
        h(
          "button",
          {
            class: "kb-btn",
            title: "What depends on this?",
            onclick: () => {
              ui.mode = "impact";
              runImpact(i);
              renderDock();
            },
          },
          icon("zap"),
          "Impact",
        ),
        h(
          "button",
          {
            class: "kb-btn",
            title: "Find how this connects to another node",
            onclick: () => {
              ui.mode = "path";
              ui.pathFrom = i;
              ui.selected = -1;
              applyFocus();
              renderInspector();
              renderDock();
              renderBanner();
            },
          },
          icon("route"),
          "Path",
        ),
      ),
      h(
        "div",
        { class: "kb-groups" },
        [...groups.entries()]
          .sort((a, b) => b[1].length - a[1].length)
          .map(([key, list]) =>
            h(
              "div",
              { class: "kb-group" },
              h("div", { class: "kb-group-head" }, key, h("span", { class: "kb-count" }, list.length)),
              list
                .slice(0, 40)
                .map(([j, e]) => nodeRow(g, j, g.conf[e] ? h("span", { class: `kb-conf kb-conf-${g.conf[e]}` }, CONFIDENCE[g.conf[e]].toLowerCase()) : h("span", { class: "kb-node-file" }, baseName(g.files[j] || "")))),
              list.length > 40 && h("div", { class: "kb-more" }, `+${list.length - 40} more`),
            ),
          ),
        hyper.length > 0 &&
          h(
            "div",
            { class: "kb-group" },
            h("div", { class: "kb-group-head" }, "Part of", h("span", { class: "kb-count" }, hyper.length)),
            hyper.map((x) =>
              h(
                "button",
                {
                  class: "kb-node-row",
                  onclick: () => {
                    ui.selected = -1;
                    ui.answer = { question: x.label, busy: false, text: "", nodes: x.nodes, group: true };
                    applyFocus();
                    galaxy.flyToNodes(x.nodes);
                    renderInspector();
                    renderAnswer();
                  },
                },
                icon("layers"),
                h("span", { class: "kb-node-name" }, x.label),
                h("span", { class: "kb-node-file" }, plural(x.nodes.length, "node")),
              ),
            ),
          ),
      ),
    );
  }

  function renderAnswer() {
    const a = ui.answer;
    const g = graph();
    answer.hidden = !a || !g || ui.tab !== "galaxy";
    shell.classList.toggle("kb-has-answer", !answer.hidden);
    if (answer.hidden) return;
    const meta = /^Traversal: (\w+).*?\| (\d+) nodes found/m.exec(a.text);
    const close = h(
      "button",
      {
        class: "kb-icon-btn",
        title: "Close",
        onclick: () => {
          ui.answer = null;
          applyFocus();
          renderAnswer();
        },
      },
      icon("x"),
    );
    fill(answer, 
      h("div", { class: "kb-panel-head" }, icon(a.group ? "layers" : "sparkles"), h("span", { class: "kb-answer-q" }, a.question), h("span", { class: "kb-grow" }), close),
      a.busy && h("div", { class: "kb-thinking" }, h("span", { class: "kb-spinner" }), "Walking the graph…"),
      a.error && h("div", { class: "kb-error" }, a.error),
      !a.busy &&
        !a.error &&
        h(
          "div",
          { class: "kb-answer-body" },
          meta && h("div", { class: "kb-subtle" }, `${meta[1]} traversal · ${meta[2]} nodes reached · ${a.nodes.length} shown`),
          !a.nodes.length && !a.group && h("div", { class: "kb-empty-small" }, "No matching nodes. Try other words, or names of functions and files."),
          h(
            "div",
            { class: "kb-chips" },
            a.nodes.slice(0, a.all ? 80 : 14).map((i) =>
              h(
                "button",
                { class: "kb-node-chip", style: `--c:${nodeCss(g, i)}`, title: fileRef(g, i), onclick: () => select(i) },
                h("span", { class: "kb-dot" }),
                g.labels[i],
              ),
            ),
          ),
          a.nodes.length > 14 &&
            !a.all &&
            h(
              "button",
              {
                class: "kb-link",
                onclick: () => {
                  a.all = true;
                  renderAnswer();
                },
              },
              `Show all ${a.nodes.length}`,
            ),
          a.text && h("details", { class: "kb-raw" }, h("summary", {}, "graphify output"), h("pre", {}, a.text)),
          !a.group &&
            h(
              "div",
              { class: "kb-actions" },
              h(
                "button",
                {
                  class: "kb-btn kb-primary",
                  onclick: () => spawnAgent(`${a.question}\n\nStart with the kb_query tool from winfer's knowledge base (this project's graphify graph), then read the code it points to.`),
                },
                icon("bot"),
                "Ask an agent",
              ),
              a.text && h("button", { class: "kb-btn", onclick: () => copy(a.text, "Answer copied") }, icon("copy"), "Copy"),
            ),
        ),
    );
  }

  function renderDock() {
    const g = graph();
    const k = current();
    dock.hidden = !g || ui.tab !== "galaxy";
    if (dock.hidden) return;
    const seg = (mode, label, ic, title) => h("button", { class: `kb-seg${ui.mode === mode ? " kb-on" : ""}`, title, onclick: () => setMode(mode) }, icon(ic), label);
    const types = [...g.typeCounts.entries()].sort((a, b) => b[1] - a[1]);
    const confCounts = [0, 0, 0];
    for (let e = 0; e < g.m; e++) confCounts[g.conf[e]]++;
    const toggle = (on, ic, title, fn) => h("button", { class: `kb-tool${on ? " kb-on" : ""}`, title, onclick: fn }, icon(ic));
    const maxDegree = Math.min(40, g.degree.reduce((m, d) => Math.max(m, d), 0));
    const degreeValue = h("span", { class: "kb-range-v" }, ui.minDegree ? `≥ ${ui.minDegree}` : "all");
    const parts = [
      h("div", { class: "kb-segs" }, seg("explore", "Explore", "orbit", "Click a node to inspect it"), seg("path", "Path", "route", "Click two nodes to see how they connect"), seg("impact", "Impact", "zap", "Click a node to see what depends on it")),
      h("span", { class: "kb-sep" }),
      h(
        "div",
        { class: "kb-filter-group" },
        types.map(([t, n]) =>
          h(
            "button",
            {
              class: `kb-filter${ui.hiddenTypes.has(t) ? "" : " kb-on"}`,
              title: `${ui.hiddenTypes.has(t) ? "Show" : "Hide"} ${typeLabel(t).toLowerCase()}`,
              onclick: () => {
                if (ui.hiddenTypes.has(t)) ui.hiddenTypes.delete(t);
                else ui.hiddenTypes.add(t);
                applyFilters();
                renderDock();
              },
            },
            h("span", { class: "kb-glyph" }, typeGlyph(t)),
            typeLabel(t),
            h("span", { class: "kb-filter-n" }, n.toLocaleString()),
          ),
        ),
      ),
      h("span", { class: "kb-sep kb-sep-conf" }),
      h(
        "div",
        { class: "kb-filter-group kb-conf-group" },
        CONFIDENCE.map((name, c) =>
          confCounts[c]
            ? h(
                "button",
                {
                  class: `kb-filter kb-conf-filter-${c}${ui.conf[c] ? " kb-on" : ""}`,
                  title: c === 0 ? "Links read straight from the code or docs" : c === 1 ? "Links the model inferred" : "Links graphify isn't sure about",
                  onclick: () => {
                    ui.conf = ui.conf.map((v, j) => (j === c ? !v : v));
                    applyFilters();
                    renderDock();
                  },
                },
                h("span", { class: "kb-line-glyph" }),
                name.charAt(0) + name.slice(1).toLowerCase(),
              )
            : null,
        ),
      ),
      h("span", { class: "kb-sep" }),
      h(
        "label",
        { class: "kb-range", title: "Hide nodes with fewer links" },
        h("span", { class: "kb-subtle" }, "Links"),
        h("input", {
          type: "range",
          min: "0",
          max: String(Math.max(1, maxDegree)),
          value: String(ui.minDegree),
          oninput: (e) => {
            ui.minDegree = Number(e.target.value);
            degreeValue.textContent = ui.minDegree ? `≥ ${ui.minDegree}` : "all";
            applyFilters();
          },
        }),
        degreeValue,
      ),
      h("span", { class: "kb-sep" }),
      toggle(store.settings.labels, "tag", "Labels", () => {
        store.set({ labels: !store.settings.labels });
        galaxy.setOptions({ labels: store.settings.labels });
      }),
      toggle(store.settings.motion, store.settings.motion ? "pause" : "play", store.settings.motion ? "Pause motion" : "Play motion (flowing links, slow spin)", () => {
        store.set({ motion: !store.settings.motion, flow: !store.settings.motion });
        galaxy.setOptions({ motion: store.settings.motion, flow: store.settings.flow });
      }),
      toggle(false, "crosshair", "Fit everything (double-click empty space)", () => galaxy.fit()),
    ];
    if (k?.snapshots.length) {
      timelineLabel = h("span", { class: "kb-range-v kb-timeline-v" });
      timelineSlider = h("input", {
        type: "range",
        min: "0",
        max: String(k.snapshots.length),
        value: String(ui.timeline.index < 0 ? k.snapshots.length : ui.timeline.index),
        oninput: (e) => {
          ui.timeline.playing = false;
          const v = Number(e.target.value);
          void setTimeline(v >= k.snapshots.length ? -1 : v);
        },
      });
      parts.push(
        h("span", { class: "kb-sep" }),
        h(
          "div",
          { class: "kb-timeline", title: `${plural(k.snapshots.length, "snapshot")} in ${OUT}/` },
          h("button", { class: `kb-tool${ui.timeline.playing ? " kb-on" : ""}`, title: ui.timeline.playing ? "Stop" : "Replay how the graph grew", onclick: () => void replay() }, icon(ui.timeline.playing ? "pause" : "clock")),
          timelineSlider,
          timelineLabel,
        ),
      );
    } else {
      timelineLabel = timelineSlider = null;
    }
    fill(dock, ...parts);
    updateTimelineLabel();
  }

  function renderFeed() {
    const g = graph();
    const recent = store.activity.filter((a) => a.root === ui.root && Date.now() - a.at < 20_000).slice(-4);
    feed.hidden = !g || !recent.length || ui.tab !== "galaxy";
    if (feed.hidden) return;
    fill(feed, 
      ...recent.map((a) =>
        h(
          "div",
          { class: "kb-feed-item kb-glass", style: `--c:${css(providerColor(a.provider))}` },
          h("span", { class: "kb-feed-dot" }),
          h("b", {}, a.label),
          h("span", { class: "kb-subtle" }, a.tool.replace("kb_", "")),
          h("span", { class: "kb-feed-text" }, a.summary ?? ""),
        ),
      ),
    );
  }

  // Overlay: empty states and insights ---------------------------------------------------------

  function renderOverlay() {
    const k = current();
    const gf = store.graphify;
    let content = null;
    let insights = false;
    if (k?.graph) {
      if (ui.tab === "insights") {
        content = insightsView(k);
        insights = true;
      }
    } else if (gf.state === "missing" || gf.state === "installing") content = installCard();
    else if (!k || k.status === "idle" || k.status === "loading" || gf.state === "unknown" || gf.state === "checking") content = loadingCard();
    else if (k.building) content = buildingCard(k);
    else if (k.status === "error") content = errorCard(k);
    else content = mapCard(k);
    overlay.hidden = !content;
    overlay.className = `kb-overlay${insights ? " kb-overlay-insights" : ""}`;
    fill(overlay, ...(content ? [content] : []));
    galaxy.setPaused(insights);
  }

  const hero = (title, text, ...rest) => h("div", { class: "kb-hero" }, h("div", { class: "kb-big-orb" }, h("span", {}), h("span", {}), h("span", {})), h("h1", { class: "kb-hero-title" }, title), text && h("p", { class: "kb-hero-text" }, text), ...rest);

  function loadingCard() {
    return hero("Reading the knowledge graph…", null);
  }

  /** Installs uv (if needed) and graphify after asking, showing what will run. */
  async function installGraphify() {
    const hasUv = (await store.exec("", IS_WIN ? FIND_UV_WIN : FIND_UV_UNIX, 30).catch(() => ({ output: "" }))).output.trim() !== "";
    const steps = [
      ...(hasUv ? [] : [IS_WIN ? `irm ${UV_INSTALLER_WIN} | iex` : `curl -LsSf ${UV_INSTALLER} | sh`]),
      "uv tool install graphifyy",
    ];
    const ok = await api.confirm(
      `winfer will run:\n\n${steps.join("\n")}\n\n${hasUv ? "" : "uv is a Python package manager from Astral; its installer adds ~/.local/bin to your PATH. "}graphify (from PyPI) then goes to ~/.local/bin. uv downloads Python itself if needed.`,
      { title: "Install graphify", okLabel: "Install", kind: "info" },
    );
    if (ok) await store.install();
  }

  function installCard() {
    const gf = store.graphify;
    if (gf.state === "installing") {
      return hero("Installing graphify", gf.step || "Working…", h("div", { class: "kb-progress" }, h("span", {})), h("p", { class: "kb-subtle" }, "This takes up to a minute the first time."));
    }
    const manual = IS_WIN ? [`powershell -c "irm ${UV_INSTALLER_WIN} | iex"`, INSTALL] : [`curl -LsSf ${UV_INSTALLER} | sh`, INSTALL];
    return hero(
      "Install graphify",
      "The knowledge base is built by graphify, a free open-source tool that turns code, docs, papers and images into a knowledge graph. winfer can install it for you with uv.",
      gf.installError && h("div", { class: "kb-alert kb-glass kb-install-error" }, icon("alert"), h("span", { class: "kb-grow" }, gf.installError)),
      h(
        "div",
        { class: "kb-actions kb-center" },
        h("button", { class: "kb-btn kb-primary kb-big", onclick: () => void installGraphify() }, icon("wand"), gf.installError ? "Try again" : "Install graphify"),
        h("button", { class: "kb-btn", onclick: () => void store.detect() }, icon("refresh"), "Check again"),
      ),
      h(
        "details",
        { class: "kb-manual" },
        h("summary", {}, "Install it yourself"),
        manual.map((cmd) => h("div", { class: "kb-code-row kb-glass" }, h("code", {}, cmd), h("button", { class: "kb-btn", onclick: () => copy(cmd, "Command copied") }, icon("copy"), "Copy"))),
        h("p", { class: "kb-subtle" }, "or ", h("code", {}, "pipx install graphifyy"), " · needs Python 3.10+"),
      ),
    );
  }

  function errorCard(k) {
    return hero(
      "Couldn't read the graph",
      k.error,
      h("div", { class: "kb-actions kb-center" }, h("button", { class: "kb-btn kb-primary", onclick: () => void store.load(k.root) }, icon("refresh"), "Try again"), h("button", { class: "kb-btn", onclick: () => void store.build(k.root, "quick", { force: true }) }, "Rebuild")),
    );
  }

  function buildingCard(k) {
    const agent = k.building.mode === "agent";
    return hero(
      agent ? "An agent is mapping this project" : "Mapping the project…",
      agent
        ? "It reads the code, docs and notes and works out concepts and why things are the way they are. This page fills in when the agent finishes."
        : k.building.mode === "headless"
          ? "graphify reads the code and docs with Claude through your API key. Big projects take a few minutes."
          : "graphify parses the code. This usually takes seconds.",
      h("div", { class: "kb-progress" }, h("span", {})),
      agent && h("div", { class: "kb-actions kb-center" }, h("button", { class: "kb-btn", onclick: () => store.cancelBuild(k.root) }, "Stop waiting")),
    );
  }

  function mapCard(k) {
    const ws = workspace();
    const option = (ic, title, text, tags, button) =>
      h("div", { class: "kb-option kb-glass" }, h("div", { class: "kb-option-icon" }, icon(ic)), h("div", { class: "kb-option-title" }, title), h("div", { class: "kb-option-text" }, text), h("div", { class: "kb-tags" }, tags.map((t) => h("span", { class: "kb-tag" }, t))), button);
    return hero(
      `Map ${ws?.name ?? "this project"}`,
      "Build a knowledge graph of the project once. Agents can then ask it where things live and what a change would break, and you can explore it here in 3D.",
      h(
        "div",
        { class: "kb-options" },
        option("terminal", "Quick map", "Functions, classes, files, calls and imports, read straight from the code.", ["Seconds", "No AI", "Free"], h("button", { class: "kb-btn kb-primary", onclick: () => void store.build(k.root, "quick") }, icon("zap"), "Map code")),
        option(
          "bot",
          "Deep map with an agent",
          "Adds docs, concepts, design rationale and inferred links. Your default agent runs graphify's skill in a new pane.",
          ["Minutes", "Uses your agent"],
          h("button", { class: "kb-btn kb-primary", onclick: () => void store.build(k.root, "agent", { workspaceId: ws?.id }) }, icon("sparkles"), "Start agent"),
        ),
        option(
          "key",
          "Deep map in the background",
          "The same deep map without a pane, using your Anthropic API key from the keychain.",
          ["Minutes", "API key"],
          ui.hasKey
            ? h("button", { class: "kb-btn kb-primary", onclick: () => void store.build(k.root, "headless") }, icon("wand"), "Build")
            : h("button", { class: "kb-btn", onclick: () => toggleMenu("settings") }, icon("key"), "Add API key"),
        ),
      ),
      h("p", { class: "kb-subtle kb-center" }, "Everything stays in ", h("code", {}, `${OUT}/`), " in the project folder."),
    );
  }

  function insightsView(k) {
    const g = k.graph;
    const r = k.report;
    const light = isLight();
    const confCounts = [0, 0, 0];
    for (let e = 0; e < g.m; e++) confCounts[g.conf[e]]++;
    const show = (fn) => () => {
      ui.tab = "galaxy";
      renderAll();
      fn();
    };
    const god = [...Array(g.n).keys()].sort((a, b) => g.degree[b] - g.degree[a]).slice(0, 10);
    const maxDeg = g.degree[god[0]] || 1;
    const relCounts = new Map();
    for (let e = 0; e < g.m; e++) relCounts.set(g.rel[e], (relCounts.get(g.rel[e]) ?? 0) + 1);
    const rels = [...relCounts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 8);
    const maxRel = rels[0]?.[1] ?? 1;
    const stale = k.head && g.commit && !k.head.startsWith(g.commit) && !g.commit.startsWith(k.head);
    const tile = (n, label, sub) => h("div", { class: "kb-tile" }, h("b", {}, n.toLocaleString()), h("span", {}, label), sub && h("span", { class: "kb-subtle" }, sub));
    const card = (title, ic, ...body) => h("section", { class: "kb-card kb-glass" }, h("div", { class: "kb-card-head" }, icon(ic), h("span", {}, title)), ...body);
    const findLabel = (label, file) => findNode(g, label, file || undefined);

    return h(
      "div",
      { class: "kb-insights" },
      h(
        "div",
        { class: "kb-ins-hero" },
        h("div", {}, h("div", { class: "kb-eyebrow" }, "Knowledge graph"), h("h1", { class: "kb-ins-title" }, r?.title || workspace()?.name || "Project"), h("div", { class: "kb-subtle" }, `Built ${ago(k.builtAt)}${g.commit ? ` from ${g.commit.slice(0, 8)}` : ""}${r?.date ? ` · report ${r.date}` : ""}`)),
        h("div", { class: "kb-tiles" }, tile(g.n, "nodes"), tile(g.m, "links"), tile(g.communities.length, "communities"), tile(g.hyperedges.length, "groups")),
      ),
      h(
        "div",
        { class: "kb-ins-grid" },
        card(
          "Core abstractions",
          "target",
          h("p", { class: "kb-subtle" }, "The most connected nodes: what everything else leans on."),
          god.map((i) =>
            h(
              "button",
              { class: "kb-bar-row", onclick: show(() => select(i)) },
              h("span", { class: "kb-dot", style: `--c:${nodeCss(g, i)}` }),
              h("span", { class: "kb-bar-label" }, g.labels[i]),
              h("span", { class: "kb-bar", style: `--w:${((g.degree[i] / maxDeg) * 100).toFixed(1)}%;--c:${nodeCss(g, i)}` }),
              h("span", { class: "kb-bar-n" }, g.degree[i]),
            ),
          ),
        ),
        card(
          "Link quality",
          "eye",
          h("div", { class: "kb-stack-bar" }, CONFIDENCE.map((name, c) => (confCounts[c] ? h("span", { class: `kb-stack-${c}`, style: `--w:${((confCounts[c] / g.m) * 100).toFixed(2)}%`, title: `${name}: ${confCounts[c]}` }) : null))),
          h(
            "div",
            { class: "kb-legend-row" },
            CONFIDENCE.map((name, c) => h("span", { class: `kb-key kb-key-${c}` }, h("i", {}), `${name.toLowerCase()} ${((confCounts[c] / Math.max(1, g.m)) * 100).toFixed(0)}%`)),
          ),
          h("div", { class: "kb-card-sub" }, "Relations"),
          rels.map(([rel, n]) => h("div", { class: "kb-bar-row kb-static" }, h("span", { class: "kb-bar-label" }, g.relations[rel]), h("span", { class: "kb-bar", style: `--w:${((n / maxRel) * 100).toFixed(1)}%` }), h("span", { class: "kb-bar-n" }, n.toLocaleString()))),
        ),
        r?.surprising.length > 0 &&
          card(
            "Surprising connections",
            "sparkles",
            h("p", { class: "kb-subtle" }, "Links across distant parts of the project you might not know about."),
            r.surprising.map((s) => {
              const a = findLabel(s.from, s.fromFile);
              const b = findLabel(s.to, s.toFile);
              return h(
                "button",
                {
                  class: "kb-surprise",
                  disabled: a < 0 || b < 0,
                  onclick: show(() => {
                    const route = shortestPath(g, a, b);
                    ui.mode = "path";
                    ui.path = route;
                    applyFocus();
                    galaxy.flyToNodes(route);
                    renderInspector();
                    renderDock();
                  }),
                },
                h("span", { class: "kb-surprise-a" }, s.from),
                h("span", { class: "kb-surprise-rel" }, s.relation.replace(/_/g, " "), h("span", { class: `kb-conf kb-conf-${CONFIDENCE.indexOf(s.confidence)}` }, s.confidence.toLowerCase())),
                h("span", { class: "kb-surprise-b" }, s.to),
                h("span", { class: "kb-subtle kb-surprise-files" }, `${s.fromFile} → ${s.toFile}`),
              );
            }),
          ),
        g.hyperedges.length > 0 &&
          card(
            "Groups",
            "layers",
            h("p", { class: "kb-subtle" }, "Sets of nodes that work together on one thing."),
            h(
              "div",
              { class: "kb-chips" },
              g.hyperedges.slice(0, 40).map((x) =>
                h(
                  "button",
                  {
                    class: "kb-group-chip",
                    onclick: show(() => {
                      ui.answer = { question: x.label, busy: false, text: "", nodes: x.nodes, group: true };
                      applyFocus();
                      galaxy.flyToNodes(x.nodes);
                      renderAnswer();
                    }),
                  },
                  x.label,
                  h("span", { class: "kb-filter-n" }, x.nodes.length),
                ),
              ),
            ),
          ),
        card(
          "Communities",
          "orbit",
          h("p", { class: "kb-subtle" }, "Clusters of nodes that belong together. Cohesion is how tightly they link to each other."),
          h(
            "div",
            { class: "kb-comm-grid" },
            g.communities.slice(0, 24).map((c) => {
              const cohesion = r?.cohesion.get(c.id);
              return h(
                "button",
                { class: "kb-comm-card", style: `--c:${css(communityColor(c.rank, light))}`, onclick: show(() => isolate(c.id)) },
                h("span", { class: "kb-comm-card-glow" }),
                h("span", { class: "kb-comm-card-name" }, c.name),
                h("span", { class: "kb-subtle" }, plural(c.count, "node"), cohesion !== undefined ? ` · cohesion ${cohesion.toFixed(2)}` : ""),
                cohesion !== undefined && h("span", { class: "kb-cohesion", style: `--w:${Math.min(100, cohesion * 100).toFixed(0)}%` }),
              );
            }),
          ),
        ),
        card(
          "Freshness",
          "clock",
          h(
            "div",
            { class: "kb-fresh" },
            h("div", {}, h("span", { class: "kb-subtle" }, "Graph built from"), h("code", {}, g.commit?.slice(0, 10) ?? "unknown")),
            h("div", {}, h("span", { class: "kb-subtle" }, "Project is at"), h("code", {}, k.head?.slice(0, 10) ?? "not a git repo")),
            h("div", {}, h("span", { class: "kb-subtle" }, "Snapshots"), h("b", {}, k.snapshots.length)),
          ),
          stale ? h("div", { class: "kb-alert kb-alert-info" }, icon("alert"), "The code moved on since the graph was built.") : h("div", { class: "kb-ok" }, icon("check"), "Up to date with the checked out commit."),
          h(
            "div",
            { class: "kb-actions" },
            h("button", { class: "kb-btn kb-primary", disabled: !!k.building, onclick: () => void store.build(k.root, "quick") }, icon("refresh"), "Update code graph"),
            h("button", { class: "kb-btn", disabled: !!k.building, onclick: () => void store.build(k.root, "agent", { workspaceId: workspace()?.id }) }, icon("bot"), "Deep update"),
          ),
        ),
      ),
    );
  }

  // Menus --------------------------------------------------------------------------------------

  /**
   * Shows a file of graphify-out in winfer's browser panel. `make` writes it when it's missing
   * (or every time with `always`): graphify skips graph.html for graphs over 5,000 nodes, and
   * `graphify export html` then writes a view of the communities instead.
   */
  async function openOutput(k, file, make, always = false) {
    const rel = `${OUT}/${file}`;
    try {
      const check = await store.exec(k.root, fileExists(rel), 15);
      if (always || !check.output.includes("@@YES")) {
        if (!make) throw new Error(`${rel} doesn't exist. Update the graph to write it.`);
        api.toast(`Writing ${file}…`);
        const r = await store.exec(k.root, headOf(make), 600);
        const again = await store.exec(k.root, fileExists(rel), 15);
        if (r.code !== 0 || !again.output.includes("@@YES")) throw new Error(lastLines(r.output, 4) || `graphify didn't write ${rel}.`);
      }
      const root = (await store.absolute(k.root)).replace(/[\\/]$/, "");
      api.execute({ type: "browser.open", url: fileUrl(`${root}/${rel}`), newTab: true });
    } catch (err) {
      api.toast(`Couldn't open ${file}: ${String(err?.message ?? err)}`, "error");
    }
  }

  function toggleMenu(name) {
    ui.menu = ui.menu === name ? null : name;
    renderTop();
    renderMenus();
  }

  function renderMenus() {
    const k = current();
    if (!ui.menu || !k) return fill(menus);
    const item = (ic, title, text, fn, disabled = false) =>
      h(
        "button",
        {
          class: "kb-menu-item",
          disabled,
          onclick: () => {
            ui.menu = null;
            renderMenus();
            renderTop();
            fn();
          },
        },
        icon(ic),
        h("span", { class: "kb-menu-text" }, h("b", {}, title), h("span", {}, text)),
      );
    if (ui.menu === "build") {
      const ws = workspace();
      fill(menus, 
        h(
          "div",
          { class: "kb-menu kb-glass" },
          item("refresh", "Update code graph", "graphify update · seconds, no AI", () => void store.build(k.root, "quick"), !!k.building),
          item("bot", "Deep build with an agent", "Your default agent adds docs, concepts and rationale", () => void store.build(k.root, "agent", { workspaceId: ws?.id }), !!k.building),
          item("key", "Deep build in the background", ui.hasKey ? "Uses your Anthropic API key" : "Add an API key in settings first", () => void store.build(k.root, "headless"), !!k.building || !ui.hasKey),
          item("alert", "Force update", "Keep the new graph even if it's smaller", () => void store.build(k.root, "quick", { force: true }), !!k.building || !k.graph),
          h("div", { class: "kb-menu-sep" }),
          item("external", "Open graph.html", "graphify's own 2D view, in the browser panel", () => void openOutput(k, "graph.html", `${store.bin} export html`), !k.graph),
          item("layers", "Open tree view", "Collapsible file tree with each symbol's links", () => void openOutput(k, "GRAPH_TREE.html", `${store.bin} tree`, true), !k.graph),
          item("file", "Open the report", "GRAPH_REPORT.md, in the browser panel", () => void openOutput(k, "GRAPH_REPORT.md"), !k.graph),
        ),
      );
      return;
    }
    const setting = (key, title, text, after) =>
      h(
        "label",
        { class: "kb-setting" },
        h("span", { class: "kb-menu-text" }, h("b", {}, title), h("span", {}, text)),
        h("input", {
          type: "checkbox",
          class: "wf-switch",
          checked: !!store.settings[key],
          onchange: (e) => {
            store.set({ [key]: e.target.checked });
            after?.();
          },
        }),
      );
    const keyInput = h("input", {
      class: "kb-input",
      type: "password",
      placeholder: ui.hasKey ? "Saved in the keychain · paste to replace" : "sk-ant-…",
      value: ui.keyDraft,
      oninput: (e) => (ui.keyDraft = e.target.value),
    });
    const gf = store.graphify;
    fill(menus, 
      h(
        "div",
        { class: "kb-menu kb-menu-settings kb-glass" },
        h("div", { class: "kb-menu-title" }, "Knowledge base"),
        setting("autoUpdate", "Update after agents finish", "Re-reads the code 20 s after an agent stops (no AI)"),
        setting("motion", "Motion", "Flowing links and a slow spin when idle", () => {
          store.set({ flow: store.settings.motion });
          galaxy.setOptions({ motion: store.settings.motion, flow: store.settings.flow });
          renderDock();
        }),
        setting("labels", "Labels", "Names of communities and core nodes", () => {
          galaxy.setOptions({ labels: store.settings.labels });
          renderDock();
        }),
        h("div", { class: "kb-menu-sep" }),
        h("div", { class: "kb-menu-title" }, "Anthropic API key", ui.hasKey ? h("span", { class: "wf-badge wf-accent" }, "saved") : null),
        h("div", { class: "kb-subtle" }, "Only for deep builds in the background. Kept in your keychain; agents never see it."),
        h(
          "div",
          { class: "kb-key-row" },
          keyInput,
          h(
            "button",
            {
              class: "kb-btn kb-primary",
              onclick: async () => {
                if (!ui.keyDraft.trim()) return;
                await api.vault.set(API_KEY, ui.keyDraft.trim(), { label: "Anthropic API key" });
                ui.keyDraft = "";
                ui.hasKey = true;
                api.toast("API key saved", "success");
                renderMenus();
                renderOverlay();
              },
            },
            "Save",
          ),
          ui.hasKey &&
            h(
              "button",
              {
                class: "kb-btn",
                onclick: async () => {
                  await api.vault.delete(API_KEY);
                  ui.hasKey = false;
                  renderMenus();
                  renderOverlay();
                },
              },
              "Remove",
            ),
        ),
        k.ignored === false &&
          h(
            "button",
            {
              class: "kb-menu-item",
              onclick: async () => {
                const r = await store.exec(k.root, IS_WIN ? `echo ${OUT}/>> .gitignore` : `printf '\\n%s\\n' '${OUT}/' >> .gitignore`, 15);
                if (r.code === 0) {
                  k.ignored = true;
                  api.toast(`Added ${OUT}/ to .gitignore`, "success");
                } else api.toast("Couldn't change .gitignore", "error");
                renderMenus();
              },
            },
            icon("git"),
            h("span", { class: "kb-menu-text" }, h("b", {}, `Add ${OUT}/ to .gitignore`), h("span", {}, "It's in the project folder but not ignored")),
          ),
        h("div", { class: "kb-menu-sep" }),
        h(
          "div",
          { class: "kb-graphify-row" },
          h("span", { class: `kb-status-dot${gf.state === "ok" ? " kb-ok-dot" : gf.state === "missing" ? " kb-bad-dot" : ""}` }),
          h("span", { class: "kb-grow kb-subtle", title: gf.path }, gf.step || (gf.state === "ok" ? `graphify ${gf.version} · ${gf.path}` : gf.state === "missing" ? "graphify isn't installed" : "Checking graphify…")),
          gf.state === "ok" && h("button", { class: "kb-link", disabled: !!gf.step, onclick: () => void store.install({ update: true }) }, "Update"),
          gf.state === "missing" && h("button", { class: "kb-link", onclick: () => void installGraphify() }, "Install"),
          h("button", { class: "kb-link", onclick: () => void store.detect() }, "Check"),
        ),
      ),
    );
  }

  // Search -------------------------------------------------------------------------------------

  function openSearch() {
    if (!graph()) return;
    search.hidden = false;
    searchInput.value = "";
    ui.searchIndex = 0;
    renderSearch();
    searchInput.focus();
  }

  function closeSearch() {
    search.hidden = true;
    shell.focus({ preventScroll: true });
  }

  function renderSearch() {
    const g = graph();
    if (!g) return;
    const list = searchNodes(g, searchInput.value);
    ui.searchIndex = clamp(ui.searchIndex, 0, Math.max(0, list.length - 1));
    fill(searchResults, 
      ...list.map((i, k) =>
        h(
          "button",
          {
            class: `kb-result${k === ui.searchIndex ? " kb-on" : ""}`,
            onmouseenter: () => {
              ui.searchIndex = k;
              renderSearch();
            },
            onclick: () => pickResult(i),
          },
          h("span", { class: "kb-dot", style: `--c:${nodeCss(g, i)}` }),
          h("span", { class: "kb-result-name" }, g.labels[i]),
          h("span", { class: "kb-result-meta" }, typeLabel(g.types[i])),
          h("span", { class: "kb-result-file" }, fileRef(g, i)),
        ),
      ),
      searchInput.value.trim() && !list.length ? h("div", { class: "kb-empty-small" }, "Nothing found. Press Enter to ask the graph instead.") : null,
    );
    searchResults.dataset.items = String(list.length);
    search._list = list;
  }

  /** A search result acts like clicking the node in the current mode. */
  function pickResult(i) {
    closeSearch();
    if (ui.mode === "path") {
      pathClick(i);
      if (!ui.path.length) galaxy.flyToNode(i, true);
    } else if (ui.mode === "impact") runImpact(i);
    else select(i);
    renderBanner();
  }

  searchInput.addEventListener("input", () => {
    ui.searchIndex = 0;
    renderSearch();
  });
  searchInput.addEventListener("keydown", (e) => {
    const list = search._list ?? [];
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      ui.searchIndex = clamp(ui.searchIndex + (e.key === "ArrowDown" ? 1 : -1), 0, Math.max(0, list.length - 1));
      renderSearch();
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (list.length) pickResult(list[ui.searchIndex]);
      else if (searchInput.value.trim()) {
        const text = searchInput.value;
        closeSearch();
        askInput.value = text;
        void ask(text);
      }
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeSearch();
    }
  });

  // Hover card ---------------------------------------------------------------------------------

  function showHover(i, x, y) {
    const g = graph();
    if (i < 0 || !g) {
      hoverCard.hidden = true;
      return;
    }
    const c = g.communityById.get(g.comm[i]);
    fill(hoverCard, 
      h("div", { class: "kb-hc-title" }, h("span", { class: "kb-dot", style: `--c:${nodeCss(g, i)}` }), h("span", {}, g.labels[i])),
      h("div", { class: "kb-hc-meta" }, `${typeLabel(g.types[i])} · ${c.name}`),
      g.files[i] && h("div", { class: "kb-hc-file" }, fileRef(g, i)),
      h("div", { class: "kb-hc-meta" }, plural(g.degree[i], "link"), ui.mode === "path" ? " · click to route" : ui.mode === "impact" ? " · click for impact" : ""),
    );
    hoverCard.hidden = false;
    const w = hoverCard.offsetWidth;
    const hh = hoverCard.offsetHeight;
    const left = x + 18 + w > shell.clientWidth ? x - w - 14 : x + 18;
    const topY = y + 18 + hh > shell.clientHeight ? y - hh - 12 : y + 18;
    hoverCard.style.transform = `translate(${left}px, ${topY}px)`;
  }

  // Keys, timers, wiring -----------------------------------------------------------------------

  function onKey(e) {
    if (!shell.isConnected || !graph()) return;
    // A terminal hidden behind this page can keep focus; only visible fields count as typing.
    const field = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLSelectElement;
    const typing = field && (shell.contains(e.target) || e.target.offsetParent !== null);
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
      e.preventDefault();
      e.stopPropagation();
      openSearch();
    } else if (typing) {
      if (e.key === "Escape" && e.target === askInput) askInput.blur();
    } else if (e.key === "/") {
      e.preventDefault();
      openSearch();
    } else if (e.key === "Escape") {
      if (ui.menu) toggleMenu(ui.menu);
      else if (ui.mode !== "explore") setMode("explore");
      else clearAll();
      renderBanner();
    } else if (e.key === "f" || e.key === "F") galaxy.fit();
  }

  function onPointerDown(e) {
    if (!ui.menu) return;
    if (e.target.closest?.(".kb-menu") || e.target.closest?.("[data-menu]")) return;
    ui.menu = null;
    renderMenus();
    renderTop();
  }

  // Narrow pages (the browser panel open) start with the legend folded away.
  // The nebula layer is larger than the page; never let focus scroll the page to it.
  shell.addEventListener("scroll", () => {
    shell.scrollTop = 0;
    shell.scrollLeft = 0;
  });
  const sizeObserver = new ResizeObserver(() => {
    const narrow = shell.clientWidth < 980;
    if (ui.legendAuto && ui.legendOpen === narrow) {
      ui.legendOpen = !narrow;
      renderLegend();
    }
  });
  sizeObserver.observe(shell);
  window.addEventListener("keydown", onKey, true);
  shell.addEventListener("pointerdown", onPointerDown, true);
  const unsubscribe = store.subscribe(renderAll);
  const unActivity = store.onActivity(onActivity);
  const ticker = setInterval(() => {
    // Plugins hear no workspace events, so notice a switch of the active workspace here.
    if (current()?.root !== ui.root) return renderAll();
    const k = current();
    if (k?.building) renderTop();
    if (store.activity.some((a) => Date.now() - a.at < 25_000)) renderFeed();
  }, 1000);
  const agentTimer = setInterval(() => void scanAgents(), 3500);
  const themeObserver = new MutationObserver(() => {
    setNebula(graph());
    renderLegendList();
    renderInspector();
  });
  themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["data-mode"] });

  void api.vault
    .list()
    .then((list) => {
      ui.hasKey = list.some((s) => s.key === API_KEY);
      renderOverlay();
    })
    .catch(() => {});
  if (store.graphify.state === "unknown") void store.detect();
  renderAll();
  shell.focus({ preventScroll: true });
  const k = current();
  if (k && k.status === "ready") void store.reloadIfChanged(k.root);
  void scanAgents();

  return () => {
    saveLayout();
    clearInterval(ticker);
    clearInterval(agentTimer);
    unsubscribe();
    unActivity();
    themeObserver.disconnect();
    sizeObserver.disconnect();
    window.removeEventListener("keydown", onKey, true);
    galaxy.destroy();
    shell.remove();
    style.remove();
  };
}

const CSS = `
.kb { position: absolute; inset: 0; overflow: hidden; overflow: clip; outline: none; user-select: none; font-size: 12px; color: var(--color-fg); container: kb / size;
  --kb-glass: color-mix(in srgb, var(--color-panel) 58%, transparent);
  --kb-glass-hi: color-mix(in srgb, var(--color-panel-raised) 80%, transparent);
  --kb-line: color-mix(in srgb, var(--color-fg) 11%, transparent);
  --kb-line-hi: color-mix(in srgb, var(--color-fg) 20%, transparent);
  --kb-accent: var(--color-accent);
  --kb-glow: color-mix(in srgb, var(--color-accent) 45%, transparent);
  background: radial-gradient(120% 90% at 50% -10%, #12163a 0%, #080a1a 42%, #030308 100%); }
:root[data-mode="light"] .kb { background: radial-gradient(120% 90% at 50% -10%, #ffffff 0%, #f1f2f9 48%, #e3e6f1 100%); --kb-glass: color-mix(in srgb, #ffffff 66%, transparent); --kb-glass-hi: color-mix(in srgb, #ffffff 88%, transparent); }
.kb [hidden] { display: none !important; }
.kb-i { width: 14px; height: 14px; flex-shrink: 0; }
.kb-nebula { position: absolute; inset: -25%; pointer-events: none; filter: blur(70px); opacity: .9;
  background: radial-gradient(28% 24% at 34% 46%, var(--kb-n1, transparent) 0%, transparent 70%), radial-gradient(24% 20% at 66% 58%, var(--kb-n2, transparent) 0%, transparent 70%), radial-gradient(20% 18% at 54% 32%, var(--kb-n3, transparent) 0%, transparent 70%);
  animation: kb-drift 70s ease-in-out infinite alternate; }
@keyframes kb-drift { to { transform: rotate(8deg) scale(1.08) translate(2%, -1%); } }
.kb-stage { position: absolute; inset: 0; overflow: hidden; }
.kb-canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; cursor: grab; touch-action: none; }
.kb-canvas:active { cursor: grabbing; }
.kb-vignette { position: absolute; inset: 0; pointer-events: none; background: radial-gradient(120% 100% at 50% 50%, transparent 55%, rgb(0 0 0 / .45) 100%); }
:root[data-mode="light"] .kb-vignette { background: radial-gradient(120% 100% at 50% 50%, transparent 60%, rgb(80 90 140 / .12) 100%); }
.kb-labels { position: absolute; inset: 0; pointer-events: none; overflow: hidden; }
.kb-label { position: absolute; left: 0; top: 0; translate: -50% -100%; white-space: nowrap; max-width: 260px; overflow: hidden; text-overflow: ellipsis; font-size: 11px; line-height: 1; padding: 4px 7px; border-radius: 6px; color: var(--color-fg);
  background: color-mix(in srgb, var(--color-app) 55%, transparent); border: 1px solid color-mix(in srgb, var(--kb-c) 40%, transparent); backdrop-filter: blur(6px); -webkit-backdrop-filter: blur(6px); will-change: transform; }
:root[data-mode="light"] .kb-label { background: rgb(255 255 255 / .7); }
.kb-label-top, .kb-label-near { font-size: 10px; color: var(--color-fg-muted); border-color: transparent; background: color-mix(in srgb, var(--color-app) 35%, transparent); }
.kb-label-selected, .kb-label-key, .kb-label-path { font-weight: 600; font-size: 12px; padding: 5px 9px; box-shadow: 0 0 18px color-mix(in srgb, var(--kb-c) 35%, transparent); }
.kb-constellation { font-size: 10px; font-weight: 600; letter-spacing: .2em; text-transform: uppercase; color: var(--kb-c); background: none; border: none; backdrop-filter: none; -webkit-backdrop-filter: none; text-shadow: 0 0 14px var(--kb-c), 0 0 2px rgb(0 0 0 / .8); max-width: 320px; }
:root[data-mode="light"] .kb-constellation { text-shadow: 0 0 10px rgb(255 255 255 / .9); background: none; }
.kb-label-agent { font-weight: 600; padding-left: 18px; border-color: var(--kb-c); box-shadow: 0 0 16px color-mix(in srgb, var(--kb-c) 45%, transparent); }
.kb-label-agent::before { content: ""; position: absolute; left: 7px; top: 50%; width: 6px; height: 6px; margin-top: -3px; border-radius: 50%; background: var(--kb-c); box-shadow: 0 0 8px var(--kb-c); animation: kb-blink 1.4s ease-in-out infinite; }
@keyframes kb-blink { 50% { opacity: .35; } }

.kb-glass { background: var(--kb-glass); backdrop-filter: blur(20px) saturate(1.6); -webkit-backdrop-filter: blur(20px) saturate(1.6); border: 1px solid var(--kb-line); border-radius: 12px; box-shadow: 0 12px 40px rgb(0 0 0 / .32), inset 0 1px 0 rgb(255 255 255 / .05); }
:root[data-mode="light"] .kb-glass { box-shadow: 0 10px 30px rgb(40 50 90 / .12), inset 0 1px 0 rgb(255 255 255 / .7); }
.kb-grow { flex: 1; min-width: 0; }
.kb-subtle { color: var(--color-fg-subtle); font-size: 11px; }
.kb-mono { font-family: var(--font-mono); }
.kb-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; background: var(--c); box-shadow: 0 0 8px var(--c); }
.kb-spinner { width: 12px; height: 12px; border-radius: 50%; border: 2px solid color-mix(in srgb, currentColor 25%, transparent); border-top-color: currentColor; animation: kb-spin .8s linear infinite; flex-shrink: 0; }
@keyframes kb-spin { to { transform: rotate(360deg); } }

.kb-btn { display: inline-flex; align-items: center; gap: 6px; height: 30px; padding: 0 11px; border-radius: 9px; font: inherit; font-size: 12px; font-weight: 500; white-space: nowrap; color: var(--color-fg); background: var(--kb-glass-hi); border: 1px solid var(--kb-line); cursor: pointer; transition: background .15s, border-color .15s, box-shadow .15s, transform .15s; }
.kb-btn:hover:not(:disabled) { border-color: var(--kb-line-hi); background: color-mix(in srgb, var(--color-hover) 90%, transparent); }
.kb-btn:active:not(:disabled) { transform: translateY(1px); }
.kb-btn:disabled { opacity: .45; cursor: default; }
.kb-btn.kb-glass { background: var(--kb-glass); }
.kb-btn.kb-on { border-color: color-mix(in srgb, var(--kb-accent) 50%, transparent); color: var(--kb-accent); }
.kb-primary { color: white; border-color: color-mix(in srgb, var(--kb-accent) 70%, white 10%); background: linear-gradient(180deg, color-mix(in srgb, var(--kb-accent) 92%, white 14%), var(--kb-accent)); box-shadow: 0 4px 18px var(--kb-glow), inset 0 1px 0 rgb(255 255 255 / .25); }
.kb-primary:hover:not(:disabled) { background: linear-gradient(180deg, color-mix(in srgb, var(--kb-accent) 85%, white 24%), var(--kb-accent)); border-color: color-mix(in srgb, var(--kb-accent) 70%, white 30%); box-shadow: 0 6px 24px var(--kb-glow), inset 0 1px 0 rgb(255 255 255 / .3); }
.kb-icon-only { width: 34px; padding: 0; justify-content: center; height: 34px; border-radius: 11px; }
.kb-icon-btn { display: inline-flex; align-items: center; justify-content: center; width: 24px; height: 24px; border-radius: 7px; border: none; background: transparent; color: var(--color-fg-subtle); cursor: pointer; flex-shrink: 0; }
.kb-icon-btn:hover { color: var(--color-fg); background: var(--color-hover); }
.kb-link { border: none; background: none; padding: 0; font: inherit; font-size: 11px; color: var(--kb-accent); cursor: pointer; }
.kb-link:hover { text-decoration: underline; }
.kb-split { display: flex; }
.kb-split > .kb-btn:first-child { border-radius: 11px 0 0 11px; height: 34px; padding: 0 13px; }
.kb-split-more { border-radius: 0 11px 11px 0; height: 34px; width: 28px; padding: 0; justify-content: center; border-left-color: rgb(255 255 255 / .2); }

.kb-top { position: absolute; z-index: 10; top: 12px; left: 12px; right: 12px; display: flex; align-items: center; gap: 8px; pointer-events: none; }
.kb-top > * { pointer-events: auto; }
.kb-brand { display: flex; align-items: center; gap: 9px; height: 34px; padding: 0 6px 0 10px; border-radius: 11px; }
.kb-title { font-weight: 600; font-size: 13px; letter-spacing: -.01em; }
.kb-brand-sep { width: 1px; height: 16px; background: var(--kb-line); }
.kb-ws-select { appearance: none; border: none; background: transparent; color: var(--color-fg-muted); font: inherit; font-size: 12px; padding: 4px 18px 4px 4px; border-radius: 6px; outline: none; cursor: pointer; max-width: 180px;
  background-image: linear-gradient(45deg, transparent 50%, currentColor 50%), linear-gradient(135deg, currentColor 50%, transparent 50%); background-position: right 7px top 50%, right 3px top 50%; background-size: 4px 4px; background-repeat: no-repeat; }
.kb-ws-select:hover { color: var(--color-fg); background-color: var(--color-hover); }
.kb-orb { position: relative; width: 16px; height: 16px; border-radius: 50%; flex-shrink: 0; background: conic-gradient(from 0deg, #7c8cff, #c27bff, #ff8fb1, #ffcf6e, #6fe3c1, #7c8cff); animation: kb-spin 6s linear infinite; box-shadow: 0 0 14px rgb(140 140 255 / .65); }
.kb-orb::after { content: ""; position: absolute; inset: 3px; border-radius: 50%; background: radial-gradient(circle at 35% 35%, #fff 0%, #c9ceff 30%, #2b2f6b 100%); }
.kb-stats { display: flex; align-items: center; gap: 14px; height: 34px; padding: 0 14px; border-radius: 11px; }
.kb-stat { display: inline-flex; align-items: baseline; gap: 5px; color: var(--color-fg-subtle); font-size: 11px; }
.kb-stat b { color: var(--color-fg); font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; }
.kb-chip { display: inline-flex; align-items: center; gap: 7px; height: 34px; padding: 0 12px; border-radius: 11px; font: inherit; font-size: 11px; color: var(--color-fg-muted); cursor: default; white-space: nowrap; flex-shrink: 0; }
.kb-top > * { flex-shrink: 0; }
.kb-top > .kb-grow { flex-shrink: 1; }
button.kb-chip.kb-warn { color: var(--color-warning); border-color: color-mix(in srgb, var(--color-warning) 35%, transparent); cursor: pointer; }
.kb-building { color: var(--color-fg); }
.kb-building .kb-spinner { color: var(--kb-accent); }
.kb-elapsed { font-variant-numeric: tabular-nums; color: var(--color-fg-subtle); }
.kb-tabs { display: flex; padding: 3px; gap: 2px; border-radius: 11px; height: 34px; box-sizing: border-box; }
.kb-tab { display: inline-flex; align-items: center; gap: 6px; padding: 0 11px; border-radius: 8px; border: none; background: none; font: inherit; font-size: 12px; color: var(--color-fg-muted); cursor: pointer; }
.kb-tab:hover { color: var(--color-fg); }
.kb-tab.kb-on { color: var(--color-fg); background: color-mix(in srgb, var(--color-fg) 9%, transparent); box-shadow: inset 0 1px 0 rgb(255 255 255 / .06); }

.kb-banner { position: absolute; z-index: 9; top: 56px; left: 50%; transform: translateX(-50%); display: flex; flex-direction: column; align-items: center; gap: 8px; width: min(620px, calc(100% - 640px)); min-width: 320px; pointer-events: none; }
.kb-banner > * { pointer-events: auto; }
.kb-alert { display: flex; align-items: center; gap: 9px; padding: 8px 10px 8px 12px; border-radius: 10px; font-size: 12px; line-height: 1.45; color: var(--color-danger); white-space: pre-wrap; max-height: 120px; overflow: auto; width: 100%; box-sizing: border-box; }
.kb-alert.kb-glass { border-color: color-mix(in srgb, var(--color-danger) 30%, transparent); }
.kb-alert-info { color: var(--color-warning); border-color: color-mix(in srgb, var(--color-warning) 30%, transparent) !important; }
.kb-card .kb-alert { padding: 8px 10px; border: 1px solid color-mix(in srgb, var(--color-warning) 30%, transparent); background: color-mix(in srgb, var(--color-warning) 8%, transparent); }
.kb-hint { display: inline-flex; align-items: center; gap: 8px; height: 30px; padding: 0 13px; border-radius: 999px; font-size: 12px; color: var(--color-fg-muted); animation: kb-rise .25s ease-out; }
.kb-hint .kb-i { color: var(--kb-accent); }
@keyframes kb-rise { from { opacity: 0; transform: translateY(-4px); } }

.kb-panel-head { display: flex; align-items: center; gap: 7px; min-height: 24px; }
.kb-panel-head > .kb-i { color: var(--kb-accent); }
.kb-eyebrow { font-size: 10px; font-weight: 600; text-transform: uppercase; letter-spacing: .12em; color: var(--color-fg-subtle); }
.kb-count { font-size: 10px; font-weight: 600; padding: 2px 6px; border-radius: 999px; color: var(--color-fg-muted); background: color-mix(in srgb, var(--color-fg) 8%, transparent); font-variant-numeric: tabular-nums; }
.kb-count-danger { color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 14%, transparent); }

.kb-legend { position: absolute; z-index: 6; top: 58px; right: 12px; width: 236px; max-height: calc(100% - 150px); display: flex; flex-direction: column; gap: 8px; padding: 10px 8px 8px 10px; box-sizing: border-box; animation: kb-slide-l .3s ease-out; }
.kb-legend-closed { padding: 0; width: auto; background: none; border: none; box-shadow: none; backdrop-filter: none; -webkit-backdrop-filter: none; }
@keyframes kb-slide-l { from { opacity: 0; transform: translateX(10px); } }
.kb-legend-filter, .kb-input { height: 28px; padding: 0 9px; border-radius: 8px; border: 1px solid var(--kb-line); background: color-mix(in srgb, var(--color-app) 40%, transparent); color: var(--color-fg); font: inherit; font-size: 12px; outline: none; user-select: text; min-width: 0; }
.kb-legend-filter:focus, .kb-input:focus { border-color: color-mix(in srgb, var(--kb-accent) 60%, transparent); }
.kb-legend-list { overflow-y: auto; display: flex; flex-direction: column; gap: 1px; margin-right: -4px; padding-right: 4px; }
.kb-comm { position: relative; display: flex; align-items: center; gap: 8px; min-height: 26px; padding: 0 7px; border-radius: 7px; border: none; background: none; font: inherit; font-size: 12px; color: var(--color-fg-muted); text-align: left; cursor: pointer; overflow: hidden; flex-shrink: 0; }
.kb-comm::after { content: ""; position: absolute; left: 0; bottom: 0; height: 2px; width: var(--w); background: linear-gradient(90deg, var(--c), transparent); opacity: .35; border-radius: 2px; }
.kb-comm:hover { background: color-mix(in srgb, var(--c) 12%, transparent); color: var(--color-fg); }
.kb-comm.kb-on { background: color-mix(in srgb, var(--c) 20%, transparent); color: var(--color-fg); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c) 45%, transparent); }
.kb-swatch { width: 9px; height: 9px; border-radius: 3px; flex-shrink: 0; background: var(--c); box-shadow: 0 0 10px color-mix(in srgb, var(--c) 70%, transparent); }
.kb-comm-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kb-comm-n { font-size: 10px; color: var(--color-fg-subtle); font-variant-numeric: tabular-nums; }

.kb-inspector { position: absolute; z-index: 6; top: 58px; left: 12px; width: 320px; max-height: calc(100% - 150px); overflow-y: auto; display: flex; flex-direction: column; gap: 12px; padding: 12px; box-sizing: border-box; animation: kb-slide-r .3s cubic-bezier(.2,.8,.2,1); user-select: text; }
@keyframes kb-slide-r { from { opacity: 0; transform: translateX(-12px); } }
.kb-type { display: inline-flex; align-items: center; gap: 5px; font-size: 10px; font-weight: 600; text-transform: uppercase; letter-spacing: .08em; color: var(--color-fg-subtle); }
.kb-comm-chip { display: inline-flex; align-items: center; gap: 6px; max-width: 170px; height: 22px; padding: 0 8px; border-radius: 999px; border: 1px solid color-mix(in srgb, var(--c) 35%, transparent); background: color-mix(in srgb, var(--c) 10%, transparent); color: var(--color-fg-muted); font: inherit; font-size: 11px; cursor: pointer; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.kb-comm-chip:hover { color: var(--color-fg); background: color-mix(in srgb, var(--c) 18%, transparent); }
.kb-comm-chip .kb-swatch { width: 7px; height: 7px; border-radius: 50%; }
.kb-insp-title { font-size: 17px; font-weight: 650; line-height: 1.25; letter-spacing: -.015em; word-break: break-word; }
.kb-insp-title.kb-mono { font-size: 15px; font-weight: 600; }
.kb-arrow { color: var(--kb-accent); }
.kb-insp-file { display: inline-flex; align-items: center; gap: 6px; align-self: flex-start; max-width: 100%; padding: 4px 8px; border-radius: 7px; border: 1px solid var(--kb-line); background: color-mix(in srgb, var(--color-app) 35%, transparent); color: var(--color-fg-muted); font: inherit; font-family: var(--font-mono); font-size: 11px; cursor: pointer; overflow: hidden; }
.kb-insp-file:hover { color: var(--color-fg); }
.kb-i-hover { opacity: 0; transition: opacity .15s; }
.kb-insp-file:hover .kb-i-hover { opacity: 1; }
.kb-metrics { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px; }
.kb-metrics > div { display: flex; flex-direction: column; gap: 2px; padding: 8px; border-radius: 9px; background: color-mix(in srgb, var(--color-fg) 5%, transparent); }
.kb-metrics b { font-size: 15px; font-weight: 650; font-variant-numeric: tabular-nums; }
.kb-metrics span { font-size: 10px; color: var(--color-fg-subtle); }
.kb-actions { display: flex; flex-wrap: wrap; gap: 6px; }
.kb-actions.kb-center { justify-content: center; }
.kb-groups { display: flex; flex-direction: column; gap: 10px; }
.kb-group { display: flex; flex-direction: column; gap: 1px; }
.kb-group-head { display: flex; align-items: center; gap: 6px; padding: 0 4px 4px; font-size: 10px; font-weight: 600; text-transform: uppercase; letter-spacing: .08em; color: var(--color-fg-subtle); }
.kb-node-row { display: flex; align-items: center; gap: 8px; min-height: 26px; padding: 0 6px; border-radius: 7px; border: none; background: none; font: inherit; font-size: 12px; color: var(--color-fg-muted); text-align: left; cursor: pointer; min-width: 0; }
.kb-node-row:hover { background: color-mix(in srgb, var(--color-fg) 7%, transparent); color: var(--color-fg); }
.kb-node-row > .kb-i { color: var(--color-fg-subtle); }
.kb-node-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kb-node-file { font-size: 10px; color: var(--color-fg-subtle); font-family: var(--font-mono); max-width: 110px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kb-conf { font-size: 9px; font-weight: 600; text-transform: uppercase; letter-spacing: .06em; padding: 1px 5px; border-radius: 4px; color: var(--color-fg-subtle); background: color-mix(in srgb, var(--color-fg) 8%, transparent); }
.kb-conf-2 { color: var(--color-warning); background: color-mix(in srgb, var(--color-warning) 14%, transparent); }
.kb-more { padding: 2px 6px; font-size: 11px; color: var(--color-fg-subtle); }
.kb-empty-small { padding: 10px 6px; font-size: 12px; color: var(--color-fg-subtle); line-height: 1.5; }
.kb-steps { display: flex; flex-direction: column; }
.kb-step { position: relative; display: flex; align-items: center; gap: 4px; }
.kb-step-rel { display: flex; align-items: center; gap: 6px; margin-left: 9px; padding: 3px 0 3px 12px; border-left: 1px dashed var(--kb-line-hi); font-size: 10px; color: var(--color-fg-subtle); text-transform: lowercase; }
.kb-step-rel .kb-i { width: 11px; height: 11px; color: var(--kb-accent); }
.kb-depths { display: flex; align-items: center; gap: 4px; }

.kb-seg { display: inline-flex; align-items: center; gap: 6px; height: 28px; padding: 0 10px; border-radius: 8px; border: none; background: none; font: inherit; font-size: 12px; color: var(--color-fg-muted); cursor: pointer; }
.kb-seg:hover { color: var(--color-fg); background: color-mix(in srgb, var(--color-fg) 6%, transparent); }
.kb-seg.kb-on { color: var(--color-fg); background: color-mix(in srgb, var(--kb-accent) 22%, transparent); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--kb-accent) 40%, transparent); }
.kb-seg.kb-on .kb-i { color: var(--kb-accent); }
.kb-seg-small { height: 22px; min-width: 24px; padding: 0 6px; justify-content: center; font-size: 11px; }

.kb-dock { position: absolute; z-index: 7; left: 50%; bottom: 12px; transform: translateX(-50%); display: flex; align-items: center; gap: 6px; padding: 5px; max-width: calc(100% - 24px); box-sizing: border-box; border-radius: 14px; overflow-x: auto; scrollbar-width: none; animation: kb-up .35s cubic-bezier(.2,.8,.2,1); }
@keyframes kb-up { from { opacity: 0; transform: translate(-50%, 8px); } }
.kb-segs, .kb-filter-group { display: flex; gap: 2px; flex-shrink: 0; }
.kb-sep { width: 1px; height: 20px; background: var(--kb-line); flex-shrink: 0; margin: 0 2px; }
.kb-filter { display: inline-flex; align-items: center; gap: 5px; height: 28px; padding: 0 8px; border-radius: 8px; border: none; background: none; font: inherit; font-size: 11px; color: var(--color-fg-subtle); cursor: pointer; white-space: nowrap; opacity: .55; }
.kb-filter:hover { opacity: .85; background: color-mix(in srgb, var(--color-fg) 6%, transparent); }
.kb-filter.kb-on { opacity: 1; color: var(--color-fg-muted); }
.kb-filter-n { font-size: 10px; color: var(--color-fg-subtle); font-variant-numeric: tabular-nums; }
.kb-glyph { font-size: 10px; color: var(--kb-accent); }
.kb-line-glyph { width: 14px; height: 0; border-top: 2px solid var(--color-fg-muted); }
.kb-conf-filter-1 .kb-line-glyph { opacity: .55; }
.kb-conf-filter-2 .kb-line-glyph { border-top-style: dashed; border-color: var(--color-warning); }
.kb-range { display: inline-flex; align-items: center; gap: 7px; padding: 0 6px; flex-shrink: 0; }
.kb-range input, .kb-timeline input { width: 84px; accent-color: var(--kb-accent); }
.kb-range-v { min-width: 28px; font-size: 11px; color: var(--color-fg-muted); font-variant-numeric: tabular-nums; }
.kb-tool { display: inline-flex; align-items: center; justify-content: center; width: 28px; height: 28px; border-radius: 8px; border: none; background: none; color: var(--color-fg-subtle); cursor: pointer; flex-shrink: 0; }
.kb-tool:hover { color: var(--color-fg); background: color-mix(in srgb, var(--color-fg) 6%, transparent); }
.kb-tool.kb-on { color: var(--kb-accent); }
.kb-timeline { display: inline-flex; align-items: center; gap: 6px; flex-shrink: 0; padding-right: 6px; }
.kb-timeline input { width: 120px; }
.kb-timeline-v { min-width: 76px; font-family: var(--font-mono); font-size: 10px; }

.kb-ask { position: absolute; z-index: 7; left: 50%; bottom: 66px; transform: translateX(-50%); width: min(560px, calc(100% - 600px)); min-width: 300px; height: 40px; display: flex; align-items: center; gap: 8px; padding: 0 5px 0 13px; box-sizing: border-box; border-radius: 13px;
  background: var(--kb-glass); backdrop-filter: blur(20px) saturate(1.6); -webkit-backdrop-filter: blur(20px) saturate(1.6); border: 1px solid var(--kb-line); box-shadow: 0 12px 40px rgb(0 0 0 / .32); isolation: isolate; }
.kb-ask-beam { position: absolute; inset: -1px; border-radius: inherit; padding: 1px; pointer-events: none; opacity: 0; transition: opacity .3s; z-index: -1;
  background: conic-gradient(from var(--kb-a, 0deg), transparent 0 60%, #7c8cff 72%, #ff8fb1 80%, #6fe3c1 88%, transparent 96%);
  -webkit-mask: linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0); -webkit-mask-composite: xor; mask: linear-gradient(#000 0 0) content-box exclude, linear-gradient(#000 0 0); animation: kb-beam 3s linear infinite; }
@property --kb-a { syntax: "<angle>"; inherits: false; initial-value: 0deg; }
@keyframes kb-beam { to { --kb-a: 360deg; } }
.kb-ask:focus-within .kb-ask-beam { opacity: 1; }
.kb-ask:focus-within { box-shadow: 0 12px 40px rgb(0 0 0 / .32), 0 0 30px var(--kb-glow); }
.kb-ask-icon { color: var(--kb-accent); }
.kb-ask-input { flex: 1; min-width: 0; height: 100%; border: none; background: none; outline: none; color: var(--color-fg); font: inherit; font-size: 13px; user-select: text; }
.kb-ask-input::placeholder { color: var(--color-fg-subtle); }
.kb-ask-send { display: inline-flex; align-items: center; justify-content: center; width: 30px; height: 30px; border-radius: 9px; border: none; color: white; background: var(--kb-accent); cursor: pointer; box-shadow: 0 2px 12px var(--kb-glow); }
.kb-ask-send:hover { filter: brightness(1.12); }

.kb-answer { position: absolute; z-index: 7; left: 12px; bottom: 66px; width: 340px; max-height: calc(46% - 20px); overflow-y: auto; display: flex; flex-direction: column; gap: 10px; padding: 12px; box-sizing: border-box; animation: kb-rise-up .3s cubic-bezier(.2,.8,.2,1); user-select: text; }
@keyframes kb-rise-up { from { opacity: 0; transform: translateY(8px); } }
.kb-has-answer .kb-inspector { max-height: calc(54% - 90px); }
.kb-answer-q { font-weight: 600; font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kb-answer-body { display: flex; flex-direction: column; gap: 10px; }
.kb-thinking { display: flex; align-items: center; gap: 8px; color: var(--color-fg-muted); }
.kb-thinking .kb-spinner { color: var(--kb-accent); }
.kb-error { color: var(--color-danger); font-size: 12px; white-space: pre-wrap; }
.kb-chips { display: flex; flex-wrap: wrap; gap: 5px; }
.kb-node-chip, .kb-group-chip { display: inline-flex; align-items: center; gap: 6px; max-width: 220px; height: 24px; padding: 0 9px; border-radius: 999px; border: 1px solid color-mix(in srgb, var(--c, var(--color-fg)) 30%, transparent); background: color-mix(in srgb, var(--c, var(--color-fg)) 9%, transparent); color: var(--color-fg); font: inherit; font-size: 11px; cursor: pointer; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.kb-node-chip:hover, .kb-group-chip:hover { background: color-mix(in srgb, var(--c, var(--color-fg)) 18%, transparent); }
.kb-raw summary { cursor: pointer; font-size: 11px; color: var(--color-fg-subtle); }
.kb-raw pre { margin: 6px 0 0; max-height: 180px; overflow: auto; padding: 8px; border-radius: 8px; font-family: var(--font-mono); font-size: 10.5px; line-height: 1.5; color: var(--color-fg-muted); background: color-mix(in srgb, var(--color-app) 45%, transparent); white-space: pre-wrap; }

.kb-feed { position: absolute; z-index: 6; left: 50%; bottom: 116px; transform: translateX(-50%); display: flex; flex-direction: column; align-items: center; gap: 6px; max-width: 460px; pointer-events: none; }
.kb-feed-item { display: flex; align-items: center; gap: 7px; padding: 7px 10px; border-radius: 10px; font-size: 11px; border-color: color-mix(in srgb, var(--c) 30%, transparent); animation: kb-slide-r .3s ease-out; overflow: hidden; }
.kb-feed-item b { font-weight: 600; white-space: nowrap; }
.kb-feed-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--c); box-shadow: 0 0 10px var(--c); flex-shrink: 0; animation: kb-blink 1.4s ease-in-out infinite; }
.kb-feed-text { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--color-fg-muted); }

.kb-hovercard { position: absolute; z-index: 12; left: 0; top: 0; max-width: 300px; padding: 9px 11px; border-radius: 10px; pointer-events: none; display: flex; flex-direction: column; gap: 4px; }
.kb-hc-title { display: flex; align-items: center; gap: 7px; font-weight: 600; font-size: 12.5px; word-break: break-word; }
.kb-hc-meta { font-size: 11px; color: var(--color-fg-subtle); }
.kb-hc-file { font-family: var(--font-mono); font-size: 10.5px; color: var(--color-fg-muted); word-break: break-all; }

.kb-search { position: absolute; z-index: 15; top: 58px; left: 50%; transform: translateX(-50%); width: min(560px, calc(100% - 40px)); padding: 6px; box-sizing: border-box; border-radius: 14px; animation: kb-rise .2s ease-out; }
.kb-search-row { display: flex; align-items: center; gap: 9px; padding: 0 8px; height: 40px; color: var(--color-fg-subtle); }
.kb-search-input { flex: 1; min-width: 0; border: none; background: none; outline: none; color: var(--color-fg); font: inherit; font-size: 14px; user-select: text; }
.kb-search kbd { font: inherit; font-size: 10px; padding: 2px 6px; border-radius: 5px; border: 1px solid var(--kb-line); color: var(--color-fg-subtle); }
.kb-search-results { display: flex; flex-direction: column; max-height: 380px; overflow-y: auto; }
.kb-search-results:not(:empty) { border-top: 1px solid var(--kb-line); padding-top: 4px; margin-top: 2px; }
.kb-result { display: grid; grid-template-columns: 10px minmax(0, 1fr) auto; grid-template-rows: auto auto; column-gap: 10px; align-items: center; padding: 7px 10px; border-radius: 9px; border: none; background: none; font: inherit; text-align: left; cursor: pointer; color: var(--color-fg); }
.kb-result.kb-on { background: color-mix(in srgb, var(--kb-accent) 16%, transparent); }
.kb-result .kb-dot { grid-row: 1 / 3; }
.kb-result-name { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kb-result-meta { font-size: 10px; color: var(--color-fg-subtle); text-transform: uppercase; letter-spacing: .06em; }
.kb-result-file { grid-column: 2 / 4; font-family: var(--font-mono); font-size: 10.5px; color: var(--color-fg-subtle); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.kb-menus { position: absolute; z-index: 20; top: 52px; right: 12px; }
.kb-menu { width: 300px; padding: 6px; border-radius: 13px; display: flex; flex-direction: column; gap: 1px; background: var(--kb-glass-hi); animation: kb-rise .18s ease-out; }
.kb-menu-settings { width: 330px; padding: 12px; gap: 8px; }
.kb-menu-title { display: flex; align-items: center; gap: 8px; font-size: 12px; font-weight: 600; }
.kb-menu-item { display: flex; align-items: center; gap: 10px; padding: 8px 9px; border-radius: 9px; border: none; background: none; font: inherit; text-align: left; color: var(--color-fg); cursor: pointer; }
.kb-menu-item > .kb-i { color: var(--kb-accent); width: 16px; height: 16px; }
.kb-menu-item:hover:not(:disabled) { background: color-mix(in srgb, var(--color-fg) 7%, transparent); }
.kb-menu-item:disabled { opacity: .45; cursor: default; }
.kb-menu-text { display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0; }
.kb-menu-text b { font-weight: 500; font-size: 12px; }
.kb-menu-text span { font-size: 11px; color: var(--color-fg-subtle); }
.kb-menu-sep { height: 1px; margin: 4px 2px; background: var(--kb-line); }
.kb-setting { display: flex; align-items: center; gap: 10px; cursor: pointer; }
.kb-key-row { display: flex; gap: 6px; }
.kb-key-row .kb-input { flex: 1; height: 30px; }
.kb-graphify-row { display: flex; align-items: center; gap: 8px; }
.kb-graphify-row .kb-subtle { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); font-size: 10.5px; }
.kb-status-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--color-fg-subtle); flex-shrink: 0; }
.kb-ok-dot { background: var(--color-success); box-shadow: 0 0 8px var(--color-success); }
.kb-bad-dot { background: var(--color-danger); }

.kb-overlay { position: absolute; z-index: 5; inset: 0; display: flex; align-items: center; justify-content: center; overflow-y: auto; padding: 70px 24px 24px; box-sizing: border-box; }
.kb-overlay-insights { align-items: flex-start; background: color-mix(in srgb, var(--color-app) 50%, transparent); backdrop-filter: blur(26px) saturate(1.3); -webkit-backdrop-filter: blur(26px) saturate(1.3); animation: kb-fade .3s ease-out; }
@keyframes kb-fade { from { opacity: 0; } }
.kb-hero { display: flex; flex-direction: column; align-items: center; gap: 14px; max-width: 860px; text-align: center; animation: kb-rise .5s ease-out; user-select: text; }
.kb-hero-title { margin: 8px 0 0; font-size: 30px; font-weight: 700; letter-spacing: -.03em; line-height: 1.1;
  background: linear-gradient(120deg, var(--color-fg) 20%, #9aa5ff 50%, #ff9fc0 70%, var(--color-fg) 90%); background-size: 200% 100%; -webkit-background-clip: text; background-clip: text; color: transparent; animation: kb-shine 8s ease-in-out infinite; }
@keyframes kb-shine { 50% { background-position: 100% 0; } }
.kb-hero-text { margin: 0; max-width: 560px; font-size: 13.5px; line-height: 1.6; color: var(--color-fg-muted); }
.kb-hero code, .kb-subtle code, .kb-fresh code { font-family: var(--font-mono); font-size: 11.5px; padding: 1px 6px; border-radius: 5px; background: color-mix(in srgb, var(--color-fg) 8%, transparent); color: var(--color-fg); }
.kb-big-orb { position: relative; width: 108px; height: 108px; }
.kb-big-orb span { position: absolute; inset: 0; border-radius: 50%; }
.kb-big-orb span:nth-child(1) { background: conic-gradient(from 0deg, #7c8cff, #c27bff, #ff8fb1, #ffcf6e, #6fe3c1, #7c8cff); filter: blur(18px); opacity: .75; animation: kb-spin 9s linear infinite; }
.kb-big-orb span:nth-child(2) { inset: 14px; background: conic-gradient(from 180deg, #7c8cff, #6fe3c1, #ffcf6e, #ff8fb1, #c27bff, #7c8cff); animation: kb-spin 6s linear infinite reverse; box-shadow: 0 0 40px rgb(140 150 255 / .6); }
.kb-big-orb span:nth-child(3) { inset: 22px; background: radial-gradient(circle at 35% 30%, #ffffff 0%, #d7dbff 22%, #3a3f8f 70%, #12143a 100%); box-shadow: inset 0 -6px 18px rgb(0 0 0 / .4); animation: kb-breathe 4s ease-in-out infinite; }
@keyframes kb-breathe { 50% { transform: scale(.94); } }
.kb-big { height: 38px; padding: 0 18px; font-size: 13px; border-radius: 11px; }
.kb-manual { display: flex; flex-direction: column; align-items: center; gap: 8px; margin-top: 6px; }
.kb-manual summary { cursor: pointer; font-size: 12px; color: var(--color-fg-subtle); list-style: none; }
.kb-manual summary:hover { color: var(--color-fg); }
.kb-manual[open] summary { margin-bottom: 8px; }
.kb-manual .kb-code-row { margin: 0 auto 6px; }
.kb-install-error { max-width: 520px; text-align: left; font-family: var(--font-mono); font-size: 11px; }
.kb-code-row { display: flex; align-items: center; gap: 10px; padding: 6px 6px 6px 14px; border-radius: 11px; }
.kb-code-row code { font-family: var(--font-mono); font-size: 13px; background: none; padding: 0; }
.kb-progress { width: 260px; height: 4px; border-radius: 4px; background: color-mix(in srgb, var(--color-fg) 10%, transparent); overflow: hidden; }
.kb-progress span { display: block; width: 40%; height: 100%; border-radius: 4px; background: linear-gradient(90deg, transparent, var(--kb-accent), #ff8fb1, transparent); animation: kb-indet 1.6s ease-in-out infinite; }
@keyframes kb-indet { from { transform: translateX(-100%); } to { transform: translateX(260%); } }
.kb-options { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; width: 100%; margin-top: 12px; text-align: left; }
.kb-option { position: relative; display: flex; flex-direction: column; gap: 8px; padding: 16px; border-radius: 16px; transition: transform .25s cubic-bezier(.2,.8,.2,1), border-color .25s, box-shadow .25s; overflow: hidden; }
.kb-option::before { content: ""; position: absolute; inset: 0; background: radial-gradient(80% 60% at 0% 0%, var(--kb-glow), transparent 60%); opacity: 0; transition: opacity .3s; pointer-events: none; }
.kb-option:hover { transform: translateY(-3px); border-color: color-mix(in srgb, var(--kb-accent) 40%, transparent); box-shadow: 0 18px 50px rgb(0 0 0 / .35), 0 0 0 1px color-mix(in srgb, var(--kb-accent) 20%, transparent); }
.kb-option:hover::before { opacity: .5; }
.kb-option-icon { display: flex; align-items: center; justify-content: center; width: 36px; height: 36px; border-radius: 11px; color: white; background: linear-gradient(140deg, #7c8cff, #c27bff); box-shadow: 0 6px 20px rgb(124 140 255 / .45); }
.kb-option:nth-child(2) .kb-option-icon { background: linear-gradient(140deg, #ff9a76, #d9608f); box-shadow: 0 6px 20px rgb(255 140 120 / .4); }
.kb-option:nth-child(3) .kb-option-icon { background: linear-gradient(140deg, #4fd1b5, #3a8fd9); box-shadow: 0 6px 20px rgb(80 200 190 / .4); }
.kb-option-icon .kb-i { width: 18px; height: 18px; }
.kb-option-title { font-size: 14px; font-weight: 650; }
.kb-option-text { flex: 1; font-size: 12px; line-height: 1.55; color: var(--color-fg-muted); }
.kb-option .kb-btn { align-self: flex-start; margin-top: 4px; }
.kb-tags { display: flex; flex-wrap: wrap; gap: 4px; }
.kb-tag { font-size: 10px; padding: 2px 7px; border-radius: 999px; color: var(--color-fg-muted); background: color-mix(in srgb, var(--color-fg) 8%, transparent); }
.kb-center { text-align: center; }

.kb-insights { width: min(1120px, 100%); display: flex; flex-direction: column; gap: 18px; padding-bottom: 24px; user-select: text; }
.kb-ins-hero { display: flex; align-items: flex-end; gap: 24px; flex-wrap: wrap; padding: 8px 4px 4px; }
.kb-ins-hero > div:first-child { flex: 1; min-width: 260px; display: flex; flex-direction: column; gap: 6px; }
.kb-ins-title { margin: 0; font-size: 28px; font-weight: 700; letter-spacing: -.03em; }
.kb-tiles { display: flex; gap: 10px; flex-wrap: wrap; }
.kb-tile { display: flex; flex-direction: column; gap: 2px; min-width: 96px; padding: 12px 14px; border-radius: 13px; background: var(--kb-glass); border: 1px solid var(--kb-line); }
.kb-tile b { font-size: 22px; font-weight: 700; letter-spacing: -.02em; font-variant-numeric: tabular-nums; background: linear-gradient(140deg, var(--color-fg), color-mix(in srgb, var(--kb-accent) 70%, var(--color-fg))); -webkit-background-clip: text; background-clip: text; color: transparent; }
.kb-tile span { font-size: 11px; color: var(--color-fg-subtle); }
.kb-ins-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 14px; align-items: start; }
.kb-card { display: flex; flex-direction: column; gap: 9px; padding: 15px; border-radius: 16px; animation: kb-rise .4s ease-out backwards; }
.kb-card:nth-child(2) { animation-delay: .05s; } .kb-card:nth-child(3) { animation-delay: .1s; } .kb-card:nth-child(4) { animation-delay: .15s; } .kb-card:nth-child(5) { animation-delay: .2s; } .kb-card:nth-child(6) { animation-delay: .25s; }
.kb-card p { margin: 0; }
.kb-card-head { display: flex; align-items: center; gap: 8px; font-size: 13px; font-weight: 650; }
.kb-card-head .kb-i { color: var(--kb-accent); width: 16px; height: 16px; }
.kb-card-sub { margin-top: 4px; font-size: 10px; font-weight: 600; text-transform: uppercase; letter-spacing: .1em; color: var(--color-fg-subtle); }
.kb-bar-row { display: grid; grid-template-columns: auto minmax(0, 1.2fr) minmax(0, 1fr) 36px; align-items: center; gap: 9px; min-height: 26px; padding: 0 6px; border-radius: 7px; border: none; background: none; font: inherit; font-size: 12px; color: var(--color-fg-muted); text-align: left; cursor: pointer; }
.kb-bar-row.kb-static { grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr) 46px; cursor: default; }
button.kb-bar-row:hover { background: color-mix(in srgb, var(--color-fg) 6%, transparent); color: var(--color-fg); }
.kb-bar-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kb-bar { position: relative; height: 6px; border-radius: 6px; background: color-mix(in srgb, var(--color-fg) 7%, transparent); overflow: hidden; }
.kb-bar::after { content: ""; position: absolute; inset: 0 auto 0 0; width: var(--w); border-radius: 6px; background: linear-gradient(90deg, color-mix(in srgb, var(--c, var(--kb-accent)) 50%, transparent), var(--c, var(--kb-accent))); box-shadow: 0 0 10px color-mix(in srgb, var(--c, var(--kb-accent)) 60%, transparent); animation: kb-grow .9s cubic-bezier(.2,.8,.2,1) backwards; transform-origin: left; }
@keyframes kb-grow { from { transform: scaleX(0); } }
.kb-bar-n { font-size: 11px; text-align: right; color: var(--color-fg-subtle); font-variant-numeric: tabular-nums; }
.kb-stack-bar { display: flex; height: 10px; border-radius: 10px; overflow: hidden; background: color-mix(in srgb, var(--color-fg) 7%, transparent); }
.kb-stack-bar span { width: var(--w); animation: kb-grow 1s cubic-bezier(.2,.8,.2,1) backwards; transform-origin: left; }
.kb-stack-0 { background: linear-gradient(90deg, #7c8cff, #9f7bff); }
.kb-stack-1 { background: linear-gradient(90deg, #ff8fb1, #ffb38a); }
.kb-stack-2 { background: repeating-linear-gradient(45deg, var(--color-warning) 0 4px, transparent 4px 8px); }
.kb-legend-row { display: flex; gap: 14px; flex-wrap: wrap; }
.kb-key { display: inline-flex; align-items: center; gap: 6px; font-size: 11px; color: var(--color-fg-muted); }
.kb-key i { width: 8px; height: 8px; border-radius: 3px; }
.kb-key-0 i { background: #8a85ff; } .kb-key-1 i { background: #ff9fa3; } .kb-key-2 i { background: var(--color-warning); }
.kb-surprise { display: grid; grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr); align-items: center; gap: 4px 10px; padding: 10px; border-radius: 11px; border: 1px solid var(--kb-line); background: color-mix(in srgb, var(--color-app) 25%, transparent); font: inherit; font-size: 12px; text-align: left; color: var(--color-fg); cursor: pointer; transition: border-color .2s, transform .2s; }
.kb-surprise:hover:not(:disabled) { border-color: color-mix(in srgb, var(--kb-accent) 40%, transparent); transform: translateY(-1px); }
.kb-surprise:disabled { cursor: default; opacity: .7; }
.kb-surprise-a, .kb-surprise-b { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 550; }
.kb-surprise-b { text-align: right; }
.kb-surprise-rel { display: inline-flex; align-items: center; gap: 5px; font-size: 10.5px; color: var(--kb-accent); white-space: nowrap; }
.kb-surprise-rel::before, .kb-surprise-rel::after { content: ""; width: 14px; height: 1px; background: linear-gradient(90deg, transparent, var(--kb-accent)); }
.kb-surprise-files { grid-column: 1 / 4; font-family: var(--font-mono); font-size: 10px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kb-comm-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 8px; }
.kb-comm-card { position: relative; display: flex; flex-direction: column; gap: 4px; padding: 11px; border-radius: 11px; border: 1px solid color-mix(in srgb, var(--c) 25%, transparent); background: color-mix(in srgb, var(--c) 6%, transparent); font: inherit; text-align: left; color: var(--color-fg); cursor: pointer; overflow: hidden; transition: transform .2s, border-color .2s; }
.kb-comm-card:hover { transform: translateY(-2px); border-color: color-mix(in srgb, var(--c) 55%, transparent); }
.kb-comm-card-glow { position: absolute; width: 60px; height: 60px; right: -20px; top: -20px; border-radius: 50%; background: var(--c); filter: blur(24px); opacity: .45; }
.kb-comm-card-name { position: relative; font-size: 12px; font-weight: 600; line-height: 1.35; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.kb-cohesion { position: relative; height: 3px; border-radius: 3px; background: color-mix(in srgb, var(--color-fg) 8%, transparent); margin-top: 3px; }
.kb-cohesion::after { content: ""; position: absolute; inset: 0 auto 0 0; width: var(--w); border-radius: 3px; background: var(--c); }
.kb-fresh { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
.kb-fresh > div { display: flex; flex-direction: column; gap: 5px; align-items: flex-start; }
.kb-ok { display: flex; align-items: center; gap: 7px; font-size: 12px; color: var(--color-success); }

@container kb (max-width: 1120px) { .kb-stats { display: none; } }
@container kb (max-width: 1340px) { .kb-dock .kb-conf-group, .kb-dock .kb-sep-conf, .kb-range > .kb-subtle { display: none; } }
@container kb (max-width: 1000px) {
  .kb-ask { width: calc(100% - 40px); min-width: 0; }
  .kb-answer { width: min(340px, calc(100% - 24px)); }
  .kb-tab-label, .kb-btn-label, .kb-chip-text { display: none; }
  .kb-tab { padding: 0 9px; }
  .kb-chip { padding: 0 10px; }
  .kb-options { grid-template-columns: 1fr; max-width: 420px; }
  .kb-banner { width: calc(100% - 40px); min-width: 0; }
  .kb-dock .kb-conf-group, .kb-dock .kb-sep-conf { display: none; }
}
@container kb (max-width: 760px) {
  .kb-title, .kb-brand-sep, .kb-dock .kb-filter-group, .kb-dock .kb-range, .kb-dock .kb-sep { display: none; }
  .kb-inspector { width: calc(100% - 24px); max-height: 45%; }
  .kb-feed { display: none; }
}
@media (prefers-reduced-motion: reduce) { .kb *, .kb *::before, .kb *::after { animation-duration: 0s !important; transition-duration: 0s !important; } }
`;

// Agent tools and activation ---------------------------------------------------------------------

/** The parts of GRAPH_REPORT.md an agent asked for. */
function reportSection(md, section) {
  const parts = md.split(/^(?=## )/m);
  const head = parts[0];
  const pick = (names) => parts.filter((p) => names.some((n) => p.toLowerCase().startsWith(`## ${n}`)));
  const sections = {
    overview: ["corpus", "summary", "graph freshness"],
    god_nodes: ["god nodes"],
    surprising: ["surprising"],
    groups: ["hyperedges"],
    communities: ["communities"],
  };
  if (section === "all") return parts.filter((p) => !p.startsWith("## Community Hubs")).join("");
  return [head, ...pick(sections[section] ?? sections.overview)].join("\n");
}

export function activate(api) {
  const store = createStore(api);
  const rootOf = (workspaceId) => api.state().workspaces.find((w) => w.id === workspaceId)?.rootDir;
  const activeRoot = () => rootOf(api.state().activeWorkspaceId);
  const noGraph = (root) =>
    `This project has no knowledge graph yet (no ${OUT}/graph.json in ${root}). Ask the user to build it from winfer's Knowledge page, or run \`graphify update .\` in the project folder for a code-only graph.`;

  /** Runs graphify for an agent in its workspace's project folder; returns what the agent sees. */
  async function graphify(caller, tool, command, { record = true, timeoutSec = 90, summary = "" } = {}) {
    const root = rootOf(caller.workspaceId) ?? caller.cwd;
    const r = await store.exec(root, ifGraph(headOf(command)), timeoutSec);
    const out = r.output.trim();
    if (out.includes("@@NOGRAPH")) throw new Error(noGraph(root));
    if (r.timedOut) throw new Error("graphify took too long and was stopped.");
    if (r.code !== 0) {
      if (r.code === 127 || /command not found|not recognized/i.test(out)) throw new Error("graphify isn't installed. Ask the user to install it from winfer's Knowledge page (one click).");
      throw new Error(out || `graphify failed (exit code ${r.code}).`);
    }
    if (record) store.record({ root, workspaceId: caller.workspaceId, paneId: caller.paneId, provider: caller.provider, label: caller.label, tool, output: out, summary });
    return truncate(out, TOOL_OUTPUT_MAX);
  }

  const text = (v) => String(v ?? "").trim();
  const need = (v, name) => {
    const s = text(v);
    if (!s) throw new Error(`${name} is required.`);
    return s;
  };

  api.registerAgentTool({
    name: "kb_query",
    description:
      "Ask this project's knowledge graph (built by graphify from its code and docs) a question in plain words. Returns the most relevant nodes (functions, classes, files, docs, concepts) with file:line, their community and how they connect. Use it before searching or reading code to find where things live and how they relate.",
    inputSchema: {
      type: "object",
      properties: {
        question: { type: "string", description: "What you want to know, e.g. 'how does the plugin store verify downloads'" },
        budget: { type: "number", description: "Max tokens of output (default 1500)" },
        depth_first: { type: "boolean", description: "Trace one path deeply instead of a broad neighborhood" },
      },
      required: ["question"],
    },
    run: (args, caller) => {
      const question = need(args.question, "question");
      const budget = clamp(Math.round(Number(args.budget) || 1500), 200, 6000);
      return graphify(caller, "kb_query", `${store.bin} query ${q(question)} --budget ${budget}${args.depth_first ? " --dfs" : ""}`, { summary: question });
    },
  });

  api.registerAgentTool({
    name: "kb_explain",
    description:
      "Explain one node of the project's knowledge graph by its label (a function like 'showToast()', a class, a file like 'utils.js', or a concept): where it's defined, its community, and every connection with relation and confidence.",
    inputSchema: { type: "object", properties: { node: { type: "string", description: "The node's label as kb_query shows it" } }, required: ["node"] },
    run: (args, caller) => {
      const node = need(args.node, "node");
      return graphify(caller, "kb_explain", `${store.bin} explain ${q(node)}`, { summary: node });
    },
  });

  api.registerAgentTool({
    name: "kb_path",
    description: "Shortest path between two nodes of the project's knowledge graph: how one function, file or concept connects to another, hop by hop with each relation.",
    inputSchema: {
      type: "object",
      properties: { from: { type: "string", description: "Start node label" }, to: { type: "string", description: "End node label" } },
      required: ["from", "to"],
    },
    run: (args, caller) => {
      const from = need(args.from, "from");
      const to = need(args.to, "to");
      return graphify(caller, "kb_path", `${store.bin} path ${q(from)} ${q(to)}`, { summary: `${from} → ${to}` });
    },
  });

  api.registerAgentTool({
    name: "kb_affected",
    description:
      "What depends on a node of the project's knowledge graph (callers, importers, subclasses, users), by depth, with file:line. Check it before changing a function, class or file to see what the change could break.",
    inputSchema: {
      type: "object",
      properties: { node: { type: "string", description: "The node's label" }, depth: { type: "number", description: "How many hops back to follow (1–4, default 2)" } },
      required: ["node"],
    },
    run: (args, caller) => {
      const node = need(args.node, "node");
      const depth = clamp(Math.round(Number(args.depth) || 2), 1, 4);
      return graphify(caller, "kb_affected", `${store.bin} affected ${q(node)} --depth ${depth}`, { summary: node });
    },
  });

  api.registerAgentTool({
    name: "kb_report",
    description:
      "Read the knowledge graph's report on the project: overview (size, freshness), god_nodes (core abstractions), surprising (unexpected cross-module links), groups (sets that work together) or communities (the project's areas and what's in them). A good first look at an unfamiliar codebase.",
    inputSchema: {
      type: "object",
      properties: { section: { type: "string", enum: ["overview", "god_nodes", "surprising", "groups", "communities", "all"], description: "Default overview" } },
    },
    run: async (args, caller) => {
      const root = rootOf(caller.workspaceId) ?? caller.cwd;
      let md;
      try {
        md = await store.readText(root, `${OUT}/GRAPH_REPORT.md`);
      } catch {
        throw new Error(noGraph(root));
      }
      return truncate(reportSection(md, text(args.section) || "overview"), TOOL_OUTPUT_MAX);
    },
  });

  api.registerAgentTool({
    name: "kb_save",
    description: "Save a question you answered about this project (and the nodes it relied on) to the knowledge graph's memory, so later questions build on it.",
    inputSchema: {
      type: "object",
      properties: {
        question: { type: "string" },
        answer: { type: "string" },
        nodes: { type: "array", items: { type: "string" }, description: "Labels of the nodes the answer cites" },
      },
      required: ["question", "answer"],
    },
    run: async (args, caller) => {
      const nodes = Array.isArray(args.nodes) ? args.nodes.map(text).filter(Boolean).slice(0, 50) : [];
      await graphify(caller, "kb_save", `${store.bin} save-result --question ${q(need(args.question, "question"))} --answer ${q(need(args.answer, "answer"))} --type query${nodes.length ? ` --nodes ${nodes.map(q).join(" ")}` : ""}`, { record: false });
      return "Saved to the knowledge graph's memory.";
    },
  });

  api.registerAgentTool({
    name: "kb_status",
    description: "Whether this project has a knowledge graph, how big it is, when it was built and whether the code changed since.",
    inputSchema: { type: "object", properties: {} },
    run: async (_args, caller) => {
      const root = rootOf(caller.workspaceId) ?? caller.cwd;
      const r = await store.exec(root, ifGraph(IS_WIN ? `type ${OUT}\\GRAPH_REPORT.md` : `echo "@@HEAD $(git rev-parse HEAD 2>/dev/null)"; sed -n '1,16p' ${OUT}/GRAPH_REPORT.md`), 30);
      if (r.output.includes("@@NOGRAPH")) return noGraph(root);
      const head = /@@HEAD ([0-9a-f]+)/.exec(r.output)?.[1];
      const report = r.output.replace(/^@@HEAD.*\n/m, "");
      const built = /Built from commit: `([0-9a-f]+)`/.exec(report)?.[1];
      const summary = reportSection(report, "overview").trim();
      const fresh = !head || !built ? "" : head.startsWith(built) ? "\n\nThe graph matches the checked out commit." : `\n\nThe code changed since the graph was built (graph: ${built}, project: ${head.slice(0, 8)}). Results may miss recent changes; winfer updates the code graph after agents finish if the user turned that on.`;
      return truncate(`Knowledge graph in ${root}/${OUT}:\n\n${summary}${fresh}`, TOOL_OUTPUT_MAX);
    },
  });

  api.registerPage({ id: "galaxy", title: "Knowledge", icon: "database", mount: (el) => mountPage(el, store, api) });

  api.registerTool({
    id: "open",
    title: "Knowledge: open the galaxy",
    description: "Show the project's knowledge graph as a 3D galaxy",
    palette: true,
    run: () => api.openPage("galaxy"),
  });
  api.registerTool({
    id: "update",
    title: "Knowledge: update the graph",
    description: "Update the knowledge graph of this project from its code with graphify",
    palette: true,
    run: () => {
      const root = activeRoot();
      if (root) void store.build(root, "quick");
    },
  });
  api.registerTool({
    id: "deep_build",
    title: "Knowledge: deep build with an agent",
    description: "Build a full knowledge graph of the code and docs with the default agent",
    risk: "confirm",
    palette: true,
    run: () => {
      const s = api.state();
      const root = activeRoot();
      if (root) void store.build(root, "agent", { workspaceId: s.activeWorkspaceId });
    },
  });

  api.on("pane.status", (e) => {
    if (e.previous !== "working" || e.status === "working") return;
    const root = rootOf(e.workspaceId);
    if (root) store.agentFinished(root);
  });

  void store.detect();
  return () => store.dispose();
}
