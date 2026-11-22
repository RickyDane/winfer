/* ==========================================================================
   Winfer native-gpui · Context Usage Modal Overhaul Mockup
   Interactive prototype for Directions A, B, and C
   ========================================================================== */

const FIXED_KINDS = new Set(["setup", "system", "tools", "mcp", "instructions"]);

const PART_COLOR_VAR = {
  setup: "var(--c-system)",
  system: "var(--c-system)",
  tools: "var(--c-tools)",
  mcp: "var(--c-mcp)",
  instructions: "var(--c-instructions)",
  messages: "var(--c-messages)",
  replies: "var(--c-replies)",
  thinking: "var(--c-thinking)",
  toolCalls: "var(--c-toolCalls)",
  toolResults: "var(--c-toolResults)",
};

const SCENARIOS = {
  early: {
    label: "Early session (Screenshot)",
    model: "Claude Opus 4.6",
    modelSpec: "claude-opus-4-6",
    context: 36_000,
    window: 1_000_000,
    input: 171_300,
    cached: 159_200,
    output: 830,
    turnOut: 830,
    speed: 68,
    cost: null,
    planLabel: "Included in plan",
    trimAt: 60,
    compactAt: 85,
    parts: [
      { kind: "system", label: "System prompt", tokens: 982, sources: [] },
      { kind: "tools", label: "Tools", tokens: 12_700, sources: [] },
      { kind: "mcp", label: "MCP tools", tokens: 10_300, sources: [] },
      { kind: "instructions", label: "Memory & instructions", tokens: 9_400, sources: [] },
      { kind: "messages", label: "Your messages", tokens: 29, sources: [] },
      { kind: "replies", label: "Replies", tokens: 48, sources: [] },
      { kind: "toolCalls", label: "Tool calls", tokens: 413, sources: [] },
      {
        kind: "toolResults",
        label: "Tool results",
        tokens: 2_000,
        sources: [{ label: "Bash", tokens: 2_000 }],
      },
    ],
  },
  active: {
    label: "Active coding session",
    model: "Claude Sonnet 4.6",
    modelSpec: "claude-sonnet-4-6",
    context: 612_400,
    window: 1_000_000,
    input: 2_840_000,
    cached: 2_584_000,
    output: 31_400,
    turnOut: 2_420,
    speed: 82,
    cost: 1.84,
    planLabel: null,
    trimAt: 60,
    compactAt: 85,
    parts: [
      { kind: "system", label: "System prompt", tokens: 4_200, sources: [] },
      { kind: "tools", label: "Tools", tokens: 14_800, sources: [] },
      { kind: "mcp", label: "MCP tools", tokens: 11_400, sources: [] },
      { kind: "instructions", label: "Memory & instructions", tokens: 12_600, sources: [] },
      { kind: "messages", label: "Your messages", tokens: 18_400, sources: [] },
      { kind: "replies", label: "Replies", tokens: 64_200, sources: [] },
      { kind: "thinking", label: "Thinking", tokens: 78_900, sources: [] },
      { kind: "toolCalls", label: "Tool calls", tokens: 42_100, sources: [] },
      {
        kind: "toolResults",
        label: "Tool results",
        tokens: 365_800,
        sources: [
          { label: "Read", tokens: 162_400 },
          { label: "Bash", tokens: 108_200 },
          { label: "Grep", tokens: 51_600 },
          { label: "Edit", tokens: 26_100 },
          { label: "Other", tokens: 17_500 },
        ],
      },
    ],
  },
  critical: {
    label: "Near context limit",
    model: "GPT-5.4 Codex",
    modelSpec: "gpt-5.4-codex",
    context: 178_400,
    window: 200_000,
    input: 1_920_000,
    cached: 1_516_000,
    output: 24_900,
    turnOut: 3_180,
    speed: 59,
    cost: 2.91,
    planLabel: null,
    trimAt: 65,
    compactAt: 85,
    parts: [
      { kind: "system", label: "System prompt", tokens: 2_600, sources: [] },
      { kind: "tools", label: "Tools", tokens: 9_800, sources: [] },
      { kind: "mcp", label: "MCP tools", tokens: 7_200, sources: [] },
      { kind: "instructions", label: "Memory & instructions", tokens: 6_400, sources: [] },
      { kind: "messages", label: "Your messages", tokens: 8_900, sources: [] },
      { kind: "replies", label: "Replies", tokens: 22_400, sources: [] },
      { kind: "thinking", label: "Thinking", tokens: 14_700, sources: [] },
      { kind: "toolCalls", label: "Tool calls", tokens: 16_200, sources: [] },
      {
        kind: "toolResults",
        label: "Tool results",
        tokens: 90_200,
        sources: [
          { label: "Bash", tokens: 46_800 },
          { label: "Read", tokens: 28_900 },
          { label: "Grep", tokens: 9_600 },
          { label: "Other", tokens: 4_900 },
        ],
      },
    ],
  },
};

const PITCHES = {
  a: "<strong>A · Cockpit Split</strong> — Radial segmented gauge + Fixed vs. Reclaimable two-column breakdown, hover cross-highlighting, cache efficiency strip, and inline compaction drawer. Built strictly with GPUI primitives (canvas arc, flex cards, Lucide icons).",
  b: "<strong>B · Zoomed Anatomy</strong> — Solves the 4% scale problem: a trapezoid zoom bridge expands the tiny used slice of the 1M ruler into a full-width 100% composition ribbon, paired with a sortable/groupable breakdown table.",
  c: "<strong>C · Compact Tabbed</strong> — Tightest 556px footprint matching the current sheet width. Persistent capacity header + 3 focused tabs (Overview, Breakdown, Compaction) that eliminate needing a separate compaction modal.",
};

// Crisp inline SVG icons (Lucide geometry, zero CDN dependency)
const ICONS = {
  gauge: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m12 14 4-4"/><path d="M3.34 19a10 10 0 1 1 17.32 0"/></svg>`,
  coins: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="8" cy="8" r="6"/><path d="M18.09 10.37A6 6 0 1 1 10.34 18"/><path d="M7 6h1v4"/><path d="m16.71 13.88.7.71-2.82 2.82"/></svg>`,
  x: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>`,
  shrink: `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m15 15 6 6m-6-6v4.8m0-4.8h4.8"/><path d="M9 19.8V15m0 0H4.2M9 15l-6 6"/><path d="M15 4.2V9m0 0h4.8M15 9l6-6"/><path d="M9 4.2V9m0 0H4.2M9 9 3 3"/></svg>`,
  sliders: `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="21" x2="14" y1="4" y2="4"/><line x1="10" x2="3" y1="4" y2="4"/><line x1="21" x2="12" y1="12" y2="12"/><line x1="8" x2="3" y1="12" y2="12"/><line x1="21" x2="16" y1="20" y2="20"/><line x1="12" x2="3" y1="20" y2="20"/><line x1="14" x2="14" y1="2" y2="6"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="16" x2="16" y1="18" y2="22"/></svg>`,
  alert: `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/></svg>`,
  chevDown: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>`,
  chevRight: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6"/></svg>`,
  lock: `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>`,
  sparkles: `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z"/></svg>`,
};

// App state
const state = {
  variant: "a",
  scenarioKey: "early",
  theme: "gruvbox-dark",
  compare: false,
  hoverKind: null,
  drawerOpen: false,
  bSort: "grouped", // 'grouped' | 'size'
  bToolsOpen: true,
  cTab: "overview", // 'overview' | 'breakdown' | 'compaction'
  cFilter: "all", // 'all' | 'fixed' | 'conv'
  // mutable copy of current scenario (so Compact Now & threshold adjustments work live!)
  data: cloneScenario("early"),
};

function cloneScenario(key) {
  return JSON.parse(JSON.stringify(SCENARIOS[key]));
}

function fmt(n) {
  if (n == null) return "—";
  const trim = (s) => s.replace(/\.0+$/, "");
  if (n >= 1_000_000) return `${trim((n / 1_000_000).toFixed(n >= 10_000_000 ? 1 : 2))}M`;
  if (n >= 1000) return `${trim((n / 1000).toFixed(n >= 100_000 ? 0 : 1))}k`;
  return String(Math.round(n));
}

function pctLabel(part, total) {
  const p = total > 0 ? (part / total) * 100 : 0;
  if (p > 0 && p < 1) return "<1%";
  return `${Math.round(p)}%`;
}

function derived(d) {
  const partsTotal = d.parts.reduce((s, p) => s + p.tokens, 0) || 1;
  const pct = d.window ? (d.context / d.window) * 100 : 0;
  const free = Math.max(0, d.window - d.context);
  const fixedParts = d.parts.filter((p) => FIXED_KINDS.has(p.kind));
  const convParts = d.parts.filter((p) => !FIXED_KINDS.has(p.kind));
  const fixedTokens = fixedParts.reduce((s, p) => s + p.tokens, 0);
  const convTokens = convParts.reduce((s, p) => s + p.tokens, 0);
  const toolRes = d.parts.find((p) => p.kind === "toolResults")?.tokens ?? 0;
  const cacheHit = d.input > 0 ? Math.min(100, (d.cached / d.input) * 100) : 0;
  const fresh = Math.max(0, d.input - d.cached);
  const totalIO = d.input + d.output || 1;
  const maxPart = Math.max(...d.parts.map((p) => p.tokens), 1);
  return {
    partsTotal,
    pct,
    free,
    fixedParts,
    convParts,
    fixedTokens,
    convTokens,
    toolRes,
    cacheHit,
    fresh,
    totalIO,
    maxPart,
    hot: pct >= 80,
  };
}

let toastTimer = null;
function showToast(msg) {
  const el = document.getElementById("toast");
  el.textContent = msg;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    el.hidden = true;
  }, 2600);
}

function runCompactSimulation() {
  const d = state.data;
  const meta = derived(d);
  if (meta.convTokens < 500) {
    showToast("Context is mostly fixed setup — nothing heavy to compact.");
    return;
  }
  const before = d.context;
  // Simulate summarizing conversation down to ~18% of conversation size
  d.parts = d.parts.map((p) => {
    if (FIXED_KINDS.has(p.kind)) return p;
    if (p.kind === "toolResults") {
      return { ...p, tokens: Math.round(p.tokens * 0.08), sources: p.sources.slice(0, 1).map((s) => ({ label: "Summary", tokens: Math.round(p.tokens * 0.08) })) };
    }
    if (p.kind === "thinking") return { ...p, tokens: 0 };
    return { ...p, tokens: Math.max(20, Math.round(p.tokens * 0.22)) };
  }).filter((p) => p.tokens > 0);
  d.context = d.parts.reduce((s, p) => s + p.tokens, 0);
  const saved = Math.max(0, before - d.context);
  render();
  showToast(`Compacted conversation · reclaimed ${fmt(saved)} tokens (${Math.round((d.context / d.window) * 100)}% used)`);
}

/* ==========================================================================
   Shared Sub-renderers
   ========================================================================== */

function renderCapacityBar(d, meta, { tall = false } = {}) {
  const pct = Math.min(100, meta.pct);
  const segs = d.parts
    .map((p) => {
      const dim = state.hoverKind && state.hoverKind !== p.kind ? "dim" : "";
      const hl = state.hoverKind === p.kind ? "hl" : "";
      return `<span class="cap-seg ${dim} ${hl}" data-hover="${p.kind}" style="flex-grow:${p.tokens};background:${PART_COLOR_VAR[p.kind]}"></span>`;
    })
    .join("");
  const trimPin = d.trimAt > 0 ? `<i class="cap-pin" style="left:${d.trimAt}%" data-tag="trim ${d.trimAt}%"></i>` : "";
  const compactPin = d.compactAt > 0 ? `<i class="cap-pin full" style="left:${d.compactAt}%" data-tag="compact ${d.compactAt}%"></i>` : "";
  return `
    <div class="cap-bar-wrap">
      <div class="cap-track ${tall ? "tall" : ""}">
        <div class="cap-fill-clip">
          <div class="cap-fill" style="width:${Math.max(pct, 0.8)}%">${segs}</div>
        </div>
        ${trimPin}
        ${compactPin}
      </div>
    </div>
  `;
}

function renderCompactionDrawer(d) {
  const opts = [0, 30, 40, 50, 60, 65, 70, 75, 80, 85, 90];
  const renderPills = (field, cur) =>
    opts
      .map(
        (v) =>
          `<button class="pct-pill ${cur === v ? "on" : ""}" data-compact-field="${field}" data-compact-val="${v}">${v === 0 ? "Off" : `${v}%`}</button>`
      )
      .join("");
  return `
    <div class="compact-drawer">
      <div class="compact-drawer-row">
        <div class="compact-drawer-head">
          <span>Micro-compact at</span>
          <small>Trims old tool output automatically</small>
        </div>
        <div class="pct-pills">${renderPills("trimAt", d.trimAt)}</div>
      </div>
      <div class="compact-drawer-row">
        <div class="compact-drawer-head">
          <span>Full compact at</span>
          <small>Summarizes the conversation before hitting the limit</small>
        </div>
        <div class="pct-pills">${renderPills("compactAt", d.compactAt)}</div>
      </div>
    </div>
  `;
}

function renderAlertIfHot(d, meta) {
  if (!meta.hot) return "";
  return `
    <div class="alert-banner">
      ${ICONS.alert}
      <div class="grow">
        <strong>${Math.round(meta.pct)}% of context used</strong> — ${fmt(meta.free)} left. Compacting now can reclaim up to <span class="mono">~${fmt(meta.convTokens)}</span> tokens.
      </div>
    </div>
  `;
}

function renderFooter(d, meta, { showCompactionLink = true } = {}) {
  const trimText = d.trimAt ? `Trim ${d.trimAt}%` : "Trim off";
  const cmpText = d.compactAt ? `Compact ${d.compactAt}%` : "Compact off";
  const ctaClass = meta.hot ? "btn-action danger" : "btn-action primary";
  return `
    ${showCompactionLink && state.drawerOpen ? renderCompactionDrawer(d) : ""}
    <footer class="m-footer">
      <div class="footer-left">
        ${
          showCompactionLink
            ? `<button class="btn-link ${state.drawerOpen ? "open" : ""}" id="toggle-compact-drawer">
                ${ICONS.sliders}
                <span>${trimText} · ${cmpText}</span>
                ${state.drawerOpen ? ICONS.chevDown : ICONS.chevRight}
              </button>`
            : `<button class="btn-link" data-ctab="compaction">
                ${ICONS.sliders}
                <span>${trimText} · ${cmpText}</span>
                ${ICONS.chevRight}
              </button>`
        }
      </div>
      <button class="${ctaClass}" id="compact-now-btn">
        ${ICONS.shrink}
        <span>Compact now</span>
        ${meta.convTokens >= 500 ? `<span class="btn-hint mono">~${fmt(meta.convTokens)}</span>` : ""}
      </button>
    </footer>
  `;
}

/* ==========================================================================
   DIRECTION A · Cockpit Split
   ========================================================================== */

function renderDonut(d, meta) {
  const r = 56;
  const C = 2 * Math.PI * r;
  // Render segments scaled to used breakdown (so even at 4% the ring shows rich composition of what's in context,
  // while an outer subtle capacity indicator or center label clearly shows 4% of 1M!)
  // Wait — even smarter: show the actual category breakdown around the ring when usage < 15% OR show window fill?
  // Let's show composition of used context on the ring with a crisp outer hairline ring showing window fill!
  let offset = 0;
  const gap = 2.2;
  const segs = d.parts
    .filter((p) => p.tokens > 0)
    .map((p) => {
      const share = p.tokens / meta.partsTotal;
      const rawLen = share * C;
      const len = Math.max(2.5, rawLen - gap);
      const dash = `${len} ${C - len}`;
      const curOffset = -offset;
      offset += rawLen;
      const dim = state.hoverKind && state.hoverKind !== p.kind ? "dim" : "";
      const hl = state.hoverKind === p.kind ? "hl" : "";
      return `<circle class="donut-seg ${dim} ${hl}" data-hover="${p.kind}" cx="73" cy="73" r="${r}" stroke="${PART_COLOR_VAR[p.kind]}" stroke-dasharray="${dash}" stroke-dashoffset="${curOffset}" />`;
    })
    .join("");

  const hovered = state.hoverKind ? d.parts.find((p) => p.kind === state.hoverKind) : null;
  const centerTop = hovered ? fmt(hovered.tokens) : `${Math.round(meta.pct)}%`;
  const centerSub = hovered ? hovered.label : `${fmt(d.context)} / ${fmt(d.window)}`;
  const centerFoot = hovered ? `${pctLabel(hovered.tokens, meta.partsTotal)} of context` : `${fmt(meta.free)} free`;

  return `
    <div class="donut-box">
      <svg class="donut-svg" viewBox="0 0 146 146" aria-label="Context breakdown ring">
        <circle class="donut-track" cx="73" cy="73" r="${r}" />
        ${segs}
      </svg>
      <div class="donut-center" id="donut-center">
        <div class="donut-pct tnum ${!hovered && meta.hot ? "danger" : ""}" style="${hovered ? `font-size:20px;color:${PART_COLOR_VAR[hovered.kind]}` : ""}">${centerTop}</div>
        <div class="donut-sub mono">${centerSub}</div>
        <div class="donut-free mono">${centerFoot}</div>
      </div>
    </div>
  `;
}

function renderPartRowA(p, meta, groupMax) {
  const dim = state.hoverKind && state.hoverKind !== p.kind ? "dim" : "";
  const hl = state.hoverKind === p.kind ? "hl" : "";
  const w = Math.max(2, (p.tokens / groupMax) * 100);
  const sourcesHtml = p.sources?.length
    ? `<div class="source-chips">${p.sources
        .map((s) => `<span class="source-chip"><span>${s.label}</span><b class="mono">${fmt(s.tokens)}</b></span>`)
        .join("")}</div>`
    : "";
  return `
    <div class="part-row ${dim} ${hl}" data-hover="${p.kind}">
      <div class="part-top">
        <i class="p-dot" style="background:${PART_COLOR_VAR[p.kind]}"></i>
        <span class="p-label">${p.label}</span>
        <span class="p-val mono">${fmt(p.tokens)}</span>
        <span class="p-pct mono">${pctLabel(p.tokens, meta.partsTotal)}</span>
      </div>
      <div class="p-bar"><span style="width:${w}%;background:${PART_COLOR_VAR[p.kind]}"></span></div>
      ${sourcesHtml}
    </div>
  `;
}

function renderVariantA(d, meta) {
  const fixedStrip = meta.fixedParts
    .map((p) => `<i style="flex-grow:${p.tokens};background:${PART_COLOR_VAR[p.kind]}"></i>`)
    .join("");
  const convStrip = meta.convParts
    .map((p) => `<i style="flex-grow:${p.tokens};background:${PART_COLOR_VAR[p.kind]}"></i>`)
    .join("");

  const hovered = state.hoverKind ? d.parts.find((p) => p.kind === state.hoverKind) : null;
  const focusHtml = `<div class="focus-callout" id="focus-callout">${renderFocusInner(hovered, meta)}</div>`;

  const costVal = d.cost != null ? `$${d.cost.toFixed(2)}` : "Plan";
  const costSub = d.cost != null ? "API estimate" : d.planLabel || "Included";

  return `
    <div class="modal w-cockpit">
      <header class="m-header">
        <div class="m-icon-badge">${ICONS.gauge}</div>
        <div class="m-title-wrap">
          <span class="m-title">Context &amp; usage</span>
          <span class="model-pill"><i class="model-dot"></i>${d.model} · ${fmt(d.window)}</span>
        </div>
        <button class="m-close" title="Close (Esc)">${ICONS.x}</button>
      </header>

      <div class="m-body">
        ${renderAlertIfHot(d, meta)}

        <!-- Hero Card -->
        <section class="card a-hero">
          ${renderDonut(d, meta)}
          <div class="a-hero-right">
            <div>
              <div class="a-hero-head">
                <span class="sec-label">Context capacity</span>
                <span class="big-nums mono">${fmt(d.context)} <span>/ ${fmt(d.window)}</span></span>
              </div>
              <div style="margin-top:16px">
                ${renderCapacityBar(d, meta, { tall: true })}
              </div>
            </div>

            <div class="a-split-cards">
              <div class="mini-zone">
                <div class="mini-zone-top">
                  <span>Fixed setup</span>
                  ${ICONS.lock}
                </div>
                <div class="mini-zone-val">
                  <b class="mono">${fmt(meta.fixedTokens)}</b>
                  <span class="mono">${pctLabel(meta.fixedTokens, meta.partsTotal)}</span>
                </div>
                <div class="mini-strip">${fixedStrip}</div>
              </div>
              <div class="mini-zone">
                <div class="mini-zone-top">
                  <span>Conversation</span>
                  ${ICONS.sparkles}
                </div>
                <div class="mini-zone-val">
                  <b class="mono">${fmt(meta.convTokens)}</b>
                  <span class="mono">${pctLabel(meta.convTokens, meta.partsTotal)}</span>
                </div>
                <div class="mini-strip">${convStrip}</div>
              </div>
            </div>

            ${focusHtml}
          </div>
        </section>

        <!-- Grouped Breakdown Cards -->
        <section class="a-groups">
          <div class="group-card">
            <div class="group-head">
              <div class="group-head-left">
                <span class="group-title">Fixed overhead</span>
                <span class="group-sub">Sent every turn · survives compact</span>
              </div>
              <span class="group-total mono">${fmt(meta.fixedTokens)}</span>
            </div>
            <div class="part-list">
              ${meta.fixedParts.map((p) => renderPartRowA(p, meta, meta.maxPart)).join("")}
            </div>
          </div>

          <div class="group-card">
            <div class="group-head">
              <div class="group-head-left">
                <span class="group-title">Conversation</span>
                <span class="group-sub">Grows per turn · reclaimable</span>
              </div>
              <span class="group-total mono">${fmt(meta.convTokens)}</span>
            </div>
            <div class="part-list">
              ${meta.convParts.map((p) => renderPartRowA(p, meta, meta.maxPart)).join("")}
            </div>
          </div>
        </section>

        <!-- Session Telemetry Bento -->
        <section class="telemetry-grid">
          <div class="cache-card">
            <div class="cache-head">
              <span>Session tokens · <b class="mono" style="color:var(--fg)">${fmt(d.input + d.output)}</b></span>
              <span class="chip-tag good mono">${Math.round(meta.cacheHit)}% cached</span>
            </div>
            <div class="split-bar">
              <span class="s-cached" style="width:${(d.cached / meta.totalIO) * 100}%"></span>
              <span class="s-fresh" style="width:${(meta.fresh / meta.totalIO) * 100}%"></span>
              <span class="s-out" style="width:${Math.max(1.5, (d.output / meta.totalIO) * 100)}%"></span>
            </div>
            <div class="split-legend mono">
              <span><i style="background:var(--success)"></i>${fmt(d.cached)} cached</span>
              <span><i style="background:var(--accent)"></i>${fmt(meta.fresh)} fresh</span>
              <span><i style="background:var(--claude)"></i>${fmt(d.output)} out</span>
            </div>
          </div>

          <div class="kpi-tile">
            <small>This turn</small>
            <b class="mono">${fmt(d.turnOut)}</b>
            <span>output tokens</span>
          </div>

          <div class="kpi-tile">
            <small>Speed</small>
            <b class="mono">${d.speed} tok/s</b>
            <span>last turn pace</span>
          </div>

          <div class="kpi-tile">
            <small>Session cost</small>
            <b class="mono">${costVal}</b>
            <span>${costSub}</span>
          </div>
        </section>
      </div>

      ${renderFooter(d, meta)}
    </div>
  `;
}

/* ==========================================================================
   DIRECTION B · Zoomed Anatomy
   ========================================================================== */

function renderVariantB(d, meta) {
  const fillPct = Math.max(1.6, Math.min(100, meta.pct));
  const hovered = state.hoverKind ? d.parts.find((p) => p.kind === state.hoverKind) : null;

  const ribbonSegs = d.parts
    .map((p) => {
      const dim = state.hoverKind && state.hoverKind !== p.kind ? "dim" : "";
      const hl = state.hoverKind === p.kind ? "hl" : "";
      return `<span class="ribbon-seg ${dim} ${hl}" data-hover="${p.kind}" style="flex-grow:${p.tokens};background:${PART_COLOR_VAR[p.kind]}" title="${p.label}: ${fmt(p.tokens)} (${pctLabel(p.tokens, meta.partsTotal)})"></span>`;
    })
    .join("");

  const renderRowB = (p, showTag) => {
    const dim = state.hoverKind && state.hoverKind !== p.kind ? "dim" : "";
    const hl = state.hoverKind === p.kind ? "hl" : "";
    const barW = Math.max(2, (p.tokens / meta.maxPart) * 100);
    const hasSources = p.sources && p.sources.length > 0;
    const nestedHtml =
      hasSources && state.bToolsOpen
        ? `<div class="b-nested">
            ${p.sources
              .map(
                (s) => `
              <div class="b-nested-row">
                <span>${s.label}</span>
                <div class="b-bar-track"><div class="b-bar-fill" style="width:${Math.max(4, (s.tokens / p.tokens) * 100)}%;background:${PART_COLOR_VAR[p.kind]};opacity:0.75"></div></div>
                <span class="b-num mono">${fmt(s.tokens)}</span>
              </div>`
              )
              .join("")}
          </div>`
        : "";
    return `
      <div class="b-row ${dim} ${hl}" data-hover="${p.kind}" ${hasSources ? 'data-toggle-tools="1"' : ""}>
        <i class="p-dot" style="background:${PART_COLOR_VAR[p.kind]}"></i>
        <div class="b-row-label">
          <span>${p.label}</span>
          ${showTag ? `<span class="b-kind-pill">${FIXED_KINDS.has(p.kind) ? "Fixed" : "Conv"}</span>` : ""}
          ${hasSources ? `<span style="color:var(--subtle);display:inline-flex">${state.bToolsOpen ? ICONS.chevDown : ICONS.chevRight}</span>` : ""}
        </div>
        <div class="b-bar-track">
          <div class="b-bar-fill" style="width:${barW}%;background:${PART_COLOR_VAR[p.kind]}"></div>
        </div>
        <span class="b-num mono">${fmt(p.tokens)}</span>
        <span class="b-pct mono">${pctLabel(p.tokens, meta.partsTotal)}</span>
      </div>
      ${nestedHtml}
    `;
  };

  let rowsHtml = "";
  if (state.bSort === "size") {
    const sorted = [...d.parts].sort((a, b) => b.tokens - a.tokens);
    rowsHtml = sorted.map((p) => renderRowB(p, true)).join("");
  } else {
    rowsHtml = `
      <div class="b-group-divider">
        <span>Fixed setup · constant per turn</span>
        <span class="mono">${fmt(meta.fixedTokens)} · ${pctLabel(meta.fixedTokens, meta.partsTotal)}</span>
      </div>
      ${meta.fixedParts.map((p) => renderRowB(p, false)).join("")}
      <div class="b-group-divider">
        <span>Conversation · reclaimable on compact</span>
        <span class="mono">${fmt(meta.convTokens)} · ${pctLabel(meta.convTokens, meta.partsTotal)}</span>
      </div>
      ${meta.convParts.map((p) => renderRowB(p, false)).join("")}
    `;
  }

  return `
    <div class="modal w-anatomy">
      <header class="m-header">
        <div class="m-icon-badge">${ICONS.gauge}</div>
        <div class="m-title-wrap">
          <span class="m-title">Context &amp; usage</span>
          <span class="model-pill"><i class="model-dot"></i>${d.model}</span>
        </div>
        <button class="m-close" title="Close (Esc)">${ICONS.x}</button>
      </header>

      <div class="m-body">
        ${renderAlertIfHot(d, meta)}

        <!-- Dual-Scale Capacity Ruler + Zoomed Composition Ribbon -->
        <section class="card">
          <div class="b-ruler-head">
            <div class="b-big-stat">
              <strong class="mono">${fmt(d.context)}</strong>
              <span class="mono">/ ${fmt(d.window)} · ${Math.round(meta.pct)}% used</span>
            </div>
            <span class="chip-tag ${meta.hot ? "danger" : "accent"} mono">${fmt(meta.free)} free</span>
          </div>

          ${renderCapacityBar(d, meta, { tall: true })}

          <div class="b-scale-ticks mono">
            <span>0</span>
            <span>${fmt(d.window * 0.25)}</span>
            <span>${fmt(d.window * 0.5)}</span>
            <span>${fmt(d.window * 0.75)}</span>
            <span>${fmt(d.window)}</span>
          </div>

          <!-- SVG Zoom Connector expanding the used slice into 100% composition bar -->
          <svg class="zoom-bridge" viewBox="0 0 1000 22" preserveAspectRatio="none" aria-hidden="true">
            <defs>
              <linearGradient id="zbGrad" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stop-color="var(--accent)" stop-opacity="0.22" />
                <stop offset="100%" stop-color="var(--accent)" stop-opacity="0.05" />
              </linearGradient>
            </defs>
            <polygon points="0,0 ${fillPct * 10},0 1000,22 0,22" fill="url(#zbGrad)" />
            <line x1="0" y1="0" x2="0" y2="22" stroke="var(--border-strong)" stroke-width="1.5" />
            <line x1="${fillPct * 10}" y1="0" x2="1000" y2="22" stroke="var(--border-strong)" stroke-width="1.5" />
          </svg>

          <div class="ribbon-head">
            <span>Inside these <b class="mono" style="color:var(--fg)">${fmt(d.context)}</b> tokens (100% view)</span>
            <span class="mono" id="ribbon-hover-readout" style="color:${hovered ? PART_COLOR_VAR[hovered.kind] : "var(--subtle)"}">
              ${hovered ? `${hovered.label} · ${fmt(hovered.tokens)} (${pctLabel(hovered.tokens, meta.partsTotal)})` : "estimated"}
            </span>
          </div>

          <div class="ribbon-bar">${ribbonSegs}</div>
        </section>

        <!-- Unified Breakdown Table -->
        <section class="card">
          <div class="b-table-head">
            <span class="sec-label">Context breakdown</span>
            <div class="seg-switch" role="group" aria-label="Sort breakdown">
              <button class="${state.bSort === "grouped" ? "on" : ""}" data-bsort="grouped">Grouped</button>
              <button class="${state.bSort === "size" ? "on" : ""}" data-bsort="size">Largest first</button>
            </div>
          </div>
          <div class="b-rows">${rowsHtml}</div>
        </section>

        <!-- Session Telemetry Summary -->
        <section class="card-tight">
          <div class="b-session-strip">
            <div class="b-metric">
              <small>Session tokens</small>
              <b class="mono">${fmt(d.input + d.output)}</b>
            </div>
            <div class="b-metric">
              <small>Cache hit</small>
              <b class="mono" style="color:var(--success)">${Math.round(meta.cacheHit)}%</b>
            </div>
            <div class="b-metric">
              <small>Pace</small>
              <b class="mono">${d.speed} tok/s</b>
            </div>
            <div class="b-metric">
              <small>Cost</small>
              <b class="mono">${d.cost != null ? `$${d.cost.toFixed(2)}` : "Plan"}</b>
            </div>
          </div>
          <div class="split-bar" style="margin-bottom:7px">
            <span class="s-cached" style="width:${(d.cached / meta.totalIO) * 100}%"></span>
            <span class="s-fresh" style="width:${(meta.fresh / meta.totalIO) * 100}%"></span>
            <span class="s-out" style="width:${Math.max(1.5, (d.output / meta.totalIO) * 100)}%"></span>
          </div>
          <div class="split-legend mono">
            <span><i style="background:var(--success)"></i>Cached input ${fmt(d.cached)}</span>
            <span><i style="background:var(--accent)"></i>Fresh input ${fmt(meta.fresh)}</span>
            <span><i style="background:var(--claude)"></i>Output ${fmt(d.output)}</span>
          </div>
        </section>
      </div>

      ${renderFooter(d, meta)}
    </div>
  `;
}

/* ==========================================================================
   DIRECTION C · Compact Tabbed Inspector
   ========================================================================== */

function renderVariantC(d, meta) {
  let tabContent = "";

  if (state.cTab === "overview") {
    const top4 = [...d.parts].sort((a, b) => b.tokens - a.tokens).slice(0, 4);
    const fixedStrip = meta.fixedParts
      .map((p) => `<i style="flex-grow:${p.tokens};background:${PART_COLOR_VAR[p.kind]}"></i>`)
      .join("");
    const convStrip = meta.convParts
      .map((p) => `<i style="flex-grow:${p.tokens};background:${PART_COLOR_VAR[p.kind]}"></i>`)
      .join("");

    tabContent = `
      ${renderAlertIfHot(d, meta)}
      <div class="c-duo">
        <div class="c-zone-card">
          <div class="z-top"><span>Fixed overhead</span>${ICONS.lock}</div>
          <div class="z-num"><b class="mono">${fmt(meta.fixedTokens)}</b><span class="mono">${pctLabel(meta.fixedTokens, meta.partsTotal)}</span></div>
          <div class="mini-strip">${fixedStrip}</div>
          <div class="z-desc">System, tools, MCP &amp; memory</div>
        </div>
        <div class="c-zone-card">
          <div class="z-top"><span>Conversation</span>${ICONS.sparkles}</div>
          <div class="z-num"><b class="mono">${fmt(meta.convTokens)}</b><span class="mono">${pctLabel(meta.convTokens, meta.partsTotal)}</span></div>
          <div class="mini-strip">${convStrip}</div>
          <div class="z-desc">Messages, replies &amp; tool output</div>
        </div>
      </div>

      <section class="card">
        <div class="b-table-head">
          <span class="sec-label">Top context consumers</span>
          <button class="btn-link" style="height:20px;padding:0 6px;font-size:11px;color:var(--accent)" data-ctab="breakdown">See all ${d.parts.length} →</button>
        </div>
        <div class="part-list">
          ${top4.map((p) => renderPartRowA(p, meta, top4[0].tokens)).join("")}
        </div>
      </section>

      <section class="card-tight">
        <div class="b-session-strip">
          <div class="b-metric">
            <small>Cache hit</small>
            <b class="mono" style="color:var(--success)">${Math.round(meta.cacheHit)}%</b>
          </div>
          <div class="b-metric">
            <small>Input</small>
            <b class="mono">${fmt(d.input)}</b>
          </div>
          <div class="b-metric">
            <small>Output</small>
            <b class="mono">${fmt(d.output)}</b>
          </div>
          <div class="b-metric">
            <small>Cost</small>
            <b class="mono">${d.cost != null ? `$${d.cost.toFixed(2)}` : "Plan"}</b>
          </div>
        </div>
        <div class="split-bar" style="margin-bottom:6px">
          <span class="s-cached" style="width:${(d.cached / meta.totalIO) * 100}%"></span>
          <span class="s-fresh" style="width:${(meta.fresh / meta.totalIO) * 100}%"></span>
          <span class="s-out" style="width:${Math.max(1.5, (d.output / meta.totalIO) * 100)}%"></span>
        </div>
        <div class="split-legend mono">
          <span><i style="background:var(--success)"></i>${fmt(d.cached)} cached</span>
          <span><i style="background:var(--accent)"></i>${fmt(meta.fresh)} fresh</span>
          <span><i style="background:var(--claude)"></i>${fmt(d.output)} out · ${d.speed} tok/s</span>
        </div>
      </section>
    `;
  } else if (state.cTab === "breakdown") {
    const list =
      state.cFilter === "fixed"
        ? meta.fixedParts
        : state.cFilter === "conv"
        ? meta.convParts
        : d.parts;
    const listMax = Math.max(...list.map((p) => p.tokens), 1);
    const ribbonSegs = d.parts
      .map((p) => {
        const dim = state.hoverKind && state.hoverKind !== p.kind ? "dim" : "";
        return `<span class="ribbon-seg ${dim}" data-hover="${p.kind}" style="flex-grow:${p.tokens};background:${PART_COLOR_VAR[p.kind]}"></span>`;
      })
      .join("");
    tabContent = `
      <section class="card">
        <div class="c-filter-row" style="margin-bottom:10px">
          <div class="seg-switch">
            <button class="${state.cFilter === "all" ? "on" : ""}" data-cfilter="all">All (${d.parts.length})</button>
            <button class="${state.cFilter === "fixed" ? "on" : ""}" data-cfilter="fixed">Fixed · ${fmt(meta.fixedTokens)}</button>
            <button class="${state.cFilter === "conv" ? "on" : ""}" data-cfilter="conv">Conversation · ${fmt(meta.convTokens)}</button>
          </div>
          <span class="sec-label">estimated</span>
        </div>
        <div class="ribbon-bar" style="height:14px;margin-bottom:10px">${ribbonSegs}</div>
        <div class="part-list">
          ${list.map((p) => renderPartRowA(p, meta, listMax)).join("")}
        </div>
      </section>
    `;
  } else {
    const opts = [0, 30, 40, 50, 60, 65, 70, 75, 80, 85, 90];
    const pills = (field, cur) =>
      opts
        .map(
          (v) =>
            `<button class="pct-pill ${cur === v ? "on" : ""}" data-compact-field="${field}" data-compact-val="${v}">${v === 0 ? "Off" : `${v}%`}</button>`
        )
        .join("");
    tabContent = `
      <div class="c-stage-explain">
        <div class="c-stage-box">
          <b><i class="p-dot" style="background:var(--warning)"></i>Micro-compact</b>
          <p>Trims old tool results first. Currently <span class="mono" style="color:var(--fg)">${fmt(meta.toolRes)}</span> in tool results.</p>
        </div>
        <div class="c-stage-box">
          <b><i class="p-dot" style="background:var(--danger)"></i>Full compact</b>
          <p>Summarizes the conversation while preserving fixed setup (<span class="mono" style="color:var(--fg)">${fmt(meta.convTokens)}</span> eligible).</p>
        </div>
      </div>

      <section class="card c-compaction-card">
        <div class="compact-drawer-row">
          <div class="compact-drawer-head">
            <span>Micro-compact threshold</span>
            <small class="mono">${d.trimAt ? `triggers at ${fmt(d.window * (d.trimAt / 100))}` : "disabled"}</small>
          </div>
          <div class="pct-pills">${pills("trimAt", d.trimAt)}</div>
        </div>
        <div class="compact-drawer-row">
          <div class="compact-drawer-head">
            <span>Full compact threshold</span>
            <small class="mono">${d.compactAt ? `triggers at ${fmt(d.window * (d.compactAt / 100))}` : "disabled"}</small>
          </div>
          <div class="pct-pills">${pills("compactAt", d.compactAt)}</div>
        </div>
      </section>
    `;
  }

  return `
    <div class="modal w-compact">
      <header class="m-header">
        <div class="m-icon-badge">${ICONS.gauge}</div>
        <div class="m-title-wrap">
          <span class="m-title">Context &amp; usage</span>
          <span class="model-pill"><i class="model-dot"></i>${d.model}</span>
        </div>
        <button class="m-close" title="Close (Esc)">${ICONS.x}</button>
      </header>

      <div class="c-top-summary">
        <div class="c-top-row">
          <div class="c-headline">
            <strong class="mono">${fmt(d.context)}</strong>
            <span class="mono">/ ${fmt(d.window)} · ${Math.round(meta.pct)}%</span>
          </div>
          <span class="chip-tag ${meta.hot ? "danger" : "accent"} mono">${fmt(meta.free)} free</span>
        </div>
        <div style="margin-top:14px">
          ${renderCapacityBar(d, meta, { tall: true })}
        </div>
      </div>

      <nav class="c-tabs" role="tablist">
        <button class="c-tab ${state.cTab === "overview" ? "on" : ""}" data-ctab="overview">Overview</button>
        <button class="c-tab ${state.cTab === "breakdown" ? "on" : ""}" data-ctab="breakdown">
          Breakdown <span class="c-tab-count mono">${d.parts.length}</span>
        </button>
        <button class="c-tab ${state.cTab === "compaction" ? "on" : ""}" data-ctab="compaction">Compaction</button>
      </nav>

      <div class="m-body">${tabContent}</div>

      ${renderFooter(d, meta, { showCompactionLink: false })}
    </div>
  `;
}

/* ==========================================================================
   Current ("Before") Modal Reference Renderer
   ========================================================================== */

function renderBeforeModal(d, meta) {
  return `
    <div class="old-head">
      ${ICONS.coins}
      <span style="flex:1;font-size:13px">Usage</span>
      <span style="color:var(--subtle)">${ICONS.x}</span>
    </div>
    <div class="old-body">
      <div>
        <div class="old-row-head">
          <span>Context</span>
          <span class="tnum">${fmt(d.context)} / ${fmt(d.window)} · ${Math.round(meta.pct)}%</span>
        </div>
        <div class="old-bar"><span style="width:${Math.min(100, meta.pct)}%;background:${meta.pct > 85 ? "var(--danger)" : "var(--accent)"}"></span></div>
      </div>

      <div class="old-stats">
        <div class="old-stat"><small>Input</small><b>${fmt(d.input)}</b></div>
        <div class="old-stat"><small>Cached</small><b>${fmt(d.cached)}</b></div>
        <div class="old-stat"><small>Output</small><b>${fmt(d.output)}</b></div>
        <div class="old-stat"><small>Cost</small><b>${d.cost != null ? `$${d.cost.toFixed(2)}` : "—"}</b></div>
      </div>

      <div style="font-size:11.5px;color:var(--subtle)">What fills the context (estimated)</div>
      <div style="display:flex;flex-direction:column;gap:7px">
        ${d.parts
          .map((p) => {
            const share = (p.tokens / meta.partsTotal) * 100;
            const subs = (p.sources || [])
              .slice(0, 4)
              .map((s) => `<div class="old-sub"><span>${s.label}</span><span>${fmt(s.tokens)}</span></div>`)
              .join("");
            return `
              <div class="old-part">
                <div class="old-part-head">
                  <span>${p.label}</span>
                  <span class="tnum">${fmt(p.tokens)} · ${Math.round(share)}%</span>
                </div>
                <div class="old-part-bar"><span style="width:${share}%"></span></div>
                ${subs}
              </div>
            `;
          })
          .join("")}
      </div>
    </div>
  `;
}

/* ==========================================================================
   Render Loop & Event Binding
   ========================================================================== */

function render() {
  const d = state.data;
  const meta = derived(d);

  document.documentElement.setAttribute("data-theme", state.theme);
  document.getElementById("review-pitch").innerHTML = PITCHES[state.variant];

  // Compare mode
  const beforeWrap = document.getElementById("before-wrap");
  const afterTag = document.getElementById("after-tag");
  beforeWrap.hidden = !state.compare;
  afterTag.hidden = !state.compare;
  if (state.compare) {
    document.getElementById("old-modal").innerHTML = renderBeforeModal(d, meta);
  }

  // Main modal
  const mount = document.getElementById("modal-mount");
  if (state.variant === "a") mount.innerHTML = renderVariantA(d, meta);
  else if (state.variant === "b") mount.innerHTML = renderVariantB(d, meta);
  else mount.innerHTML = renderVariantC(d, meta);

  bindModalEvents(mount);
}

function renderFocusInner(hovered, meta) {
  if (!hovered) {
    return `<span class="fc-left">Hover any slice or category to inspect its share</span><span class="mono">estimated</span>`;
  }
  return `
    <span class="fc-left">
      <i class="focus-dot" style="background:${PART_COLOR_VAR[hovered.kind]}"></i>
      <strong>${hovered.label}</strong>
      <span>· ${FIXED_KINDS.has(hovered.kind) ? "Fixed per turn" : "Reclaimable"}</span>
    </span>
    <span class="mono">${fmt(hovered.tokens)} · ${pctLabel(hovered.tokens, meta.partsTotal)}</span>
  `;
}

function updateHoverVisuals(kind) {
  state.hoverKind = kind;
  const d = state.data;
  const meta = derived(d);
  const mount = document.getElementById("modal-mount");
  const hovered = kind ? d.parts.find((p) => p.kind === kind) : null;

  mount.querySelectorAll("[data-hover]").forEach((el) => {
    const k = el.dataset.hover;
    el.classList.toggle("dim", Boolean(kind && k !== kind));
    el.classList.toggle("hl", Boolean(kind && k === kind));
  });

  const dc = mount.querySelector("#donut-center");
  if (dc) {
    const top = hovered ? fmt(hovered.tokens) : `${Math.round(meta.pct)}%`;
    const sub = hovered ? hovered.label : `${fmt(d.context)} / ${fmt(d.window)}`;
    const foot = hovered ? `${pctLabel(hovered.tokens, meta.partsTotal)} of context` : `${fmt(meta.free)} free`;
    dc.innerHTML = `
      <div class="donut-pct tnum ${!hovered && meta.hot ? "danger" : ""}" style="${hovered ? `font-size:20px;color:${PART_COLOR_VAR[hovered.kind]}` : ""}">${top}</div>
      <div class="donut-sub mono">${sub}</div>
      <div class="donut-free mono">${foot}</div>
    `;
  }

  const fc = mount.querySelector("#focus-callout");
  if (fc) fc.innerHTML = renderFocusInner(hovered, meta);

  const rr = mount.querySelector("#ribbon-hover-readout");
  if (rr) {
    rr.style.color = hovered ? PART_COLOR_VAR[hovered.kind] : "var(--subtle)";
    rr.textContent = hovered ? `${hovered.label} · ${fmt(hovered.tokens)} (${pctLabel(hovered.tokens, meta.partsTotal)})` : "estimated";
  }
}

function bindModalEvents(root) {
  root.querySelectorAll("[data-hover]").forEach((el) => {
    el.addEventListener("mouseenter", () => {
      const k = el.dataset.hover;
      if (state.hoverKind !== k) updateHoverVisuals(k);
    });
    el.addEventListener("mouseleave", () => {
      if (state.hoverKind !== null) updateHoverVisuals(null);
    });
  });

  const drawerBtn = root.querySelector("#toggle-compact-drawer");
  if (drawerBtn) {
    drawerBtn.onclick = () => {
      state.drawerOpen = !state.drawerOpen;
      render();
    };
  }

  const compactBtn = root.querySelector("#compact-now-btn");
  if (compactBtn) {
    compactBtn.onclick = runCompactSimulation;
  }

  root.querySelectorAll("[data-compact-field]").forEach((btn) => {
    btn.onclick = () => {
      const field = btn.dataset.compactField;
      state.data[field] = Number(btn.dataset.compactVal);
      render();
    };
  });

  root.querySelectorAll("[data-bsort]").forEach((btn) => {
    btn.onclick = () => {
      state.bSort = btn.dataset.bsort;
      render();
    };
  });

  root.querySelectorAll("[data-toggle-tools]").forEach((el) => {
    el.onclick = () => {
      state.bToolsOpen = !state.bToolsOpen;
      render();
    };
  });

  root.querySelectorAll("[data-ctab]").forEach((btn) => {
    btn.onclick = () => {
      state.cTab = btn.dataset.ctab;
      render();
    };
  });

  root.querySelectorAll("[data-cfilter]").forEach((btn) => {
    btn.onclick = () => {
      state.cFilter = btn.dataset.cfilter;
      render();
    };
  });
}

// Wire Review Bar
document.querySelectorAll("#variant-tabs .rv-tab").forEach((btn) => {
  btn.onclick = () => {
    document.querySelectorAll("#variant-tabs .rv-tab").forEach((b) => {
      b.classList.toggle("on", b === btn);
      b.setAttribute("aria-selected", b === btn ? "true" : "false");
    });
    state.variant = btn.dataset.variant;
    state.hoverKind = null;
    state.drawerOpen = false;
    render();
  };
});

document.querySelectorAll("#scenario-tabs .rv-pill").forEach((btn) => {
  btn.onclick = () => {
    document.querySelectorAll("#scenario-tabs .rv-pill").forEach((b) => b.classList.toggle("on", b === btn));
    state.scenarioKey = btn.dataset.scenario;
    state.data = cloneScenario(state.scenarioKey);
    state.hoverKind = null;
    render();
  };
});

document.getElementById("theme-select").onchange = (e) => {
  state.theme = e.target.value;
  render();
};

document.getElementById("compare-toggle").onclick = (e) => {
  state.compare = !state.compare;
  e.currentTarget.classList.toggle("on", state.compare);
  render();
};

render();
