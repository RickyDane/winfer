// Shared data + renderers for the three Home task chat mockups.
const P = {
  home: '<rect x="3" y="3" width="7" height="9" rx="1"/><rect x="14" y="3" width="7" height="5" rx="1"/><rect x="14" y="12" width="7" height="9" rx="1"/><rect x="3" y="16" width="7" height="5" rx="1"/>',
  term: '<path d="m4 17 6-6-6-6"/><path d="M12 19h8"/>',
  file: '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>',
  edit: '<path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/>',
  check: '<path d="M20 6 9 17l-5-5"/>',
  x: '<path d="M18 6 6 18M6 6l12 12"/>',
  back: '<path d="m15 18-6-6 6-6"/>',
  chev: '<path d="m9 18 6-6-6-6"/>',
  down: '<path d="m6 9 6 6 6-6"/>',
  ext: '<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
  branch: '<circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="8" r="2.5"/><path d="M6 8.5v7M18 10.5c0 4-6 3-11 6"/>',
  server: '<rect x="3" y="3" width="18" height="7" rx="2"/><rect x="3" y="14" width="18" height="7" rx="2"/><path d="M7 6.5h.01M7 17.5h.01"/>',
  send: '<path d="M12 19V5M5 12l7-7 7 7"/>',
  bot: '<rect x="4" y="8" width="16" height="12" rx="3"/><path d="M12 4v4M9 13h.01M15 13h.01"/>',
  users: '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0"/><path d="M16 4.5a3.5 3.5 0 0 1 0 7M18 14a6 6 0 0 1 3.5 6"/>',
  shield: '<path d="M12 3 4 6v6c0 5 3.5 8 8 9 4.5-1 8-4 8-9V6z"/><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .8-1 1.5V14M12 17h.01"/>',
  more: '<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  spawn: '<path d="M12 3v6M12 15v6M3 12h6M15 12h6"/><circle cx="12" cy="12" r="3"/>',
  merge: '<circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="18" r="2.5"/><path d="M6 8.5v7M8.5 6c6 0 9.5 3 9.5 9.5"/>',
  split: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16"/>',
  thread: '<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z"/>',
  mic: '<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/>',
  restart: '<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/>',
};
const I = (n, s) => `<svg class="ic" viewBox="0 0 24 24"${s ? ` style="width:${s}px;height:${s}px"` : ''}>${P[n]}</svg>`;
const TOOL = { Read: ['file', '#7c8cff'], Grep: ['search', '#a78bfa'], Edit: ['edit', '#e08b6a'], Write: ['edit', '#e08b6a'], Bash: ['term', '#4ade80'], spawn_agent: ['spawn', '#38bdf8'], update_task: ['home', '#fbbf24'] };
const STAGE = { running: 'Running', needs: 'Needs you', ready: 'Ready', merged: 'Merged', planning: 'Planning', checking: 'Checking' };
const MT = { api: '#3987e5', web: '#d55181', core: '#199e70', winfer: '#9085e9', docs: '#c98500' };
const tile = ws => `<span class="mono-tile" style="background:${MT[ws] || '#555'}">${ws[0].toUpperCase()}</span>`;
const pill = st => `<span class="pill st-${st}">${STAGE[st]}</span>`;

// Agents: status working | needs | done | idle. pane = where it really lives (secondary action).
const TASKS = [
  { id: 't2', n: 2, goal: 'Add CSV export to the reports page', ws: 'web', mode: 'fork', stage: 'needs', model: 'Opus 4.6', ask: '2 waiting: 1 question, 1 approval',
    agents: [
      { id: 'orch', name: 'Orchestrator', role: 'plans & merges', status: 'needs', av: ['#7c8cff', 'O'], pane: 'web/1', branch: 'winfer/t2' },
      { id: 'backend', name: 'backend', role: 'API endpoint', status: 'needs', av: ['#e08b6a', 'B'], pane: 'web/3', branch: 'winfer/t2-backend' },
      { id: 'frontend', name: 'frontend', role: 'Export button', status: 'working', av: ['#d55181', 'F'], pane: 'web/4', branch: 'winfer/t2-frontend' },
      { id: 'tests', name: 'tests', role: 'e2e coverage', status: 'done', av: ['#4ade80', 'T'], pane: 'web/5', branch: 'winfer/t2-tests' },
    ] },
  { id: 't1', n: 1, goal: 'Fix the flaky login e2e test on CI', ws: 'api', mode: 'light', stage: 'running', model: 'Sonnet 4.6', step: 'Running pnpm test auth --repeat 20',
    agents: [{ id: 'a', name: 'Claude', role: 'one agent', status: 'working', av: ['#e08b6a', 'C'], pane: 'api/2', branch: 'winfer/api-2' }] },
  { id: 't3', n: 3, goal: 'Bump tokio to 1.48 and fix the deprecations', ws: 'core', mode: 'remote', machine: 'hetzner-01', stage: 'ready', model: 'Opus 4.6',
    agents: [{ id: 'orch', name: 'Orchestrator', role: 'on hetzner-01', status: 'done', av: ['#38bdf8', 'O'], pane: 'hetzner-01:core/1', branch: 'winfer/tokio-148' }] },
  { id: 't4', n: 4, goal: 'Cut the v0.42 release and watch the pipeline', ws: 'winfer', mode: 'remote', machine: 'mac-studio', stage: 'running', model: 'Sonnet 4.6', step: 'Watching release.yml · 6/9 jobs',
    agents: [{ id: 'orch', name: 'Orchestrator', role: 'on mac-studio', status: 'working', av: ['#38bdf8', 'O'], pane: 'mac-studio:winfer/2', branch: 'release/0.42' }] },
  { id: 't5', n: 5, goal: 'Dark mode toggle on the settings page', ws: 'web', mode: 'light', stage: 'merged', model: 'Sonnet 4.6',
    agents: [{ id: 'a', name: 'Claude', role: 'one agent', status: 'done', av: ['#e08b6a', 'C'], pane: 'web/2', branch: 'winfer/web-2' }] },
];
const task = id => TASKS.find(t => t.id === id);
const HOW = { light: 'one agent', fork: 'orchestrated', remote: 'on another machine' };

const CHATS = {
  't2/orch': [
    { t: 'handoff', text: 'Add CSV export to the reports page. Respect the current filters, stream big exports, cover it with an e2e test.' },
    { t: 'text', who: 'orch', html: '<p>Plan: three agents in their own worktrees off <code>main</code>.</p><ul><li><b>backend</b>: <code>GET /reports/:id/export.csv</code>, streamed</li><li><b>frontend</b>: Export button + progress toast</li><li><b>tests</b>: Playwright e2e for a 10k-row report</li></ul>' },
    { t: 'tools', rows: [{ n: 'spawn_agent', ti: 'backend · Sonnet 4.6 · winfer/t2-backend', tm: '2s', ok: 1 }, { n: 'spawn_agent', ti: 'frontend · Sonnet 4.6 · winfer/t2-frontend', tm: '2s', ok: 1 }, { n: 'spawn_agent', ti: 'tests · Haiku 4.5 · winfer/t2-tests', tm: '1s', ok: 1 }] },
    { t: 'event', text: 'tests finished · 14 min ago' },
    { t: 'text', who: 'orch', html: '<p><b>tests</b> is done (1 new spec, green on its branch). <b>backend</b> is waiting on an install approval. Before frontend wires the button I need one decision from you.</p>' },
    { t: 'diff', title: 'winfer/t2-tests', files: [['e2e/reports-export.spec.ts', 88, 0], ['e2e/fixtures/report-10k.json', 12, 0], ['playwright.config.ts', 2, 1]] },
  ],
  't2/backend': [
    { t: 'handoff', from: 'Orchestrator', text: 'Add GET /reports/:id/export.csv. Stream rows, reuse ReportQuery filters, 401 for other orgs.' },
    { t: 'tools', rows: [{ n: 'Read', ti: 'src/routes/reports.ts', tm: '0.2s', ok: 1 }, { n: 'Grep', ti: '"ReportQuery" in src/', tm: '0.4s', ok: 1 }, { n: 'Edit', ti: 'src/routes/reports.ts', a: 46, d: 3, tm: '1.1s', ok: 1 }, { n: 'Write', ti: 'src/export/csv-stream.ts', a: 61, d: 0, tm: '0.6s', ok: 1 }] },
    { t: 'text', who: 'backend', html: '<p>Endpoint and streaming writer are in. I want <code>csv-stringify</code> for correct quoting instead of hand-rolling it.</p>' },
  ],
  't2/frontend': [
    { t: 'handoff', from: 'Orchestrator', text: 'Export button on ReportHeader; show progress for long exports. Wait for the scope decision before wiring the query.' },
    { t: 'tools', rows: [{ n: 'Read', ti: 'src/components/ReportHeader.tsx', tm: '0.1s', ok: 1 }, { n: 'Edit', ti: 'src/components/ReportHeader.tsx', a: 18, d: 2, tm: '0.9s', ok: 1 }, { n: 'Write', ti: 'src/components/ExportButton.tsx', a: 54, d: 0, tm: '0.7s', ok: 1 }, { n: 'Bash', ti: 'pnpm tsc --noEmit', tm: '12s', ok: null }] },
  ],
  't2/tests': [
    { t: 'handoff', from: 'Orchestrator', text: 'Playwright e2e: export a 10k-row report, assert header + row count.' },
    { t: 'tools', rows: [{ n: 'Write', ti: 'e2e/reports-export.spec.ts', a: 88, d: 0, tm: '0.8s', ok: 1 }, { n: 'Bash', ti: 'pnpm playwright test reports-export', tm: '41s', ok: 1 }] },
    { t: 'text', who: 'tests', html: '<p>Spec passes against a mocked endpoint (3 runs, 0 flakes). Done. Ready for the orchestrator to merge.</p>' },
    { t: 'diff', title: 'winfer/t2-tests', files: [['e2e/reports-export.spec.ts', 88, 0], ['e2e/fixtures/report-10k.json', 12, 0], ['playwright.config.ts', 2, 1]] },
  ],
  't1/a': [
    { t: 'handoff', text: 'login.spec.ts fails ~1 in 8 runs on CI. Find the race and fix it, no retries.' },
    { t: 'tools', rows: [{ n: 'Grep', ti: '"waitForURL" in tests/e2e', tm: '0.3s', ok: 1 }, { n: 'Read', ti: 'tests/e2e/login.spec.ts', tm: '0.1s', ok: 1 }] },
    { t: 'text', who: 'a', html: '<p>The test clicks <i>Sign in</i> before the CSRF cookie is set, so the first POST sometimes 403s. Waiting for <code>/session/csrf</code> fixes the race without retries.</p>' },
    { t: 'tools', rows: [{ n: 'Edit', ti: 'tests/e2e/login.spec.ts', a: 7, d: 2, tm: '0.4s', ok: 1 }, { n: 'Bash', ti: 'pnpm test auth --repeat 20', tm: '38s', ok: null }] },
  ],
  't3/orch': [
    { t: 'handoff', text: 'Bump tokio to 1.48 in every crate, fix the deprecations, keep clippy clean.' },
    { t: 'tools', rows: [{ n: 'Bash', ti: 'cargo update -p tokio --precise 1.48.0', tm: '9s', ok: 1 }, { n: 'Edit', ti: '7 files · replace task::spawn_blocking_in_place', a: 52, d: 79, tm: '6s', ok: 1 }, { n: 'Bash', ti: 'cargo clippy --workspace -- -D warnings', tm: '1m 12s', ok: 1 }, { n: 'Bash', ti: 'cargo test --workspace', tm: '2m 40s', ok: 1 }] },
    { t: 'text', who: 'orch', html: '<p>All 412 tests pass, clippy is clean. The branch is ready to merge on <b>hetzner-01</b>.</p>' },
    { t: 'diff', title: 'winfer/tokio-148 · hetzner-01', files: [['Cargo.lock', 18, 22], ['crates/core/src/runtime.rs', 14, 31], ['crates/net/src/pool.rs', 9, 17], ['crates/fs/src/watch.rs', 6, 12], ['+ 4 more files', 17, 9]], merge: true },
  ],
  't4/orch': [
    { t: 'handoff', text: 'Cut v0.42: changelog from merged PRs, tag, and watch release.yml until the artifacts are up.' },
    { t: 'tools', rows: [{ n: 'Write', ti: 'CHANGELOG.md', a: 34, d: 0, tm: '0.5s', ok: 1 }, { n: 'Bash', ti: 'git tag v0.42.0 && git push --tags', tm: '3s', ok: 1 }, { n: 'Bash', ti: 'gh run watch 1189 · release.yml', tm: '8m', ok: null }] },
    { t: 'text', who: 'orch', html: '<p>6 of 9 jobs green; the notarized macOS build is next.</p>' },
  ],
  't5/a': [
    { t: 'handoff', text: 'Add a dark mode toggle to the settings page, follow the system by default.' },
    { t: 'tools', rows: [{ n: 'Edit', ti: 'src/pages/Settings.tsx', a: 22, d: 4, tm: '0.6s', ok: 1 }, { n: 'Bash', ti: 'pnpm test settings', tm: '9s', ok: 1 }] },
    { t: 'event', text: 'merged into main · yesterday 18:04' },
  ],
};
// Pending items dock above the composer, as approvals do in code_view.rs.
const PENDING = {
  't2/orch': { kind: 'question', header: 'EXPORT SCOPE', q: 'What should "Export" include?', opts: [['Current filters', 'What the user sees now (recommended)'], ['Whole report', 'Ignore filters'], ['Ask every time', 'Small dialog before export']] },
  't2/backend': { kind: 'approval', tool: 'Bash', cmd: 'pnpm add csv-stringify@6.5.2', detail: 'Adds 1 dependency to web/package.json' },
};

const AG = (t, id) => t.agents.find(a => a.id === id) || t.agents[0];
function avatar(t, id) { const a = AG(t, id); return `<span class="avatar" style="background:${a.av[0]}">${a.av[1]}</span>`; }

function toolRow(r) {
  const [ic, col] = TOOL[r.n] || ['term', '#a1a1aa'];
  const st = r.ok === null ? '<span class="spin"></span>' : `<span style="color:var(--success)">${I('check', 13)}</span>`;
  const ct = r.a != null ? `<span class="ct"><span class="add">+${r.a}</span><span class="del">-${r.d}</span></span>` : '';
  return `<div class="tool"><span class="badge" style="background:${col}24;color:${col}">${I(ic)}</span><span class="nm">${r.n}</span><span class="ti">${r.ti}</span>${ct}<span class="tm">${r.tm}</span>${st}<span class="subtle">${I('chev', 12)}</span></div>`;
}
function diffCard(d) {
  const A = d.files.reduce((s, f) => s + f[1], 0), D = d.files.reduce((s, f) => s + f[2], 0);
  const bar = (a, dd) => { const n = Math.max(1, Math.round((a + dd) / 12)), na = Math.min(5, Math.round(5 * a / ((a + dd) || 1))); return [...Array(5)].map((_, i) => `<i class="${i < na ? 'a' : i < 5 && n > 0 && dd ? 'd' : ''}"></i>`).join(''); };
  return `<div class="diff"><div class="dh">${I('branch')}<span>${d.title}</span><span class="grow"></span><span class="ct"><span class="add">+${A}</span><span class="del">-${D}</span></span><span class="subtle">${d.files.length} files</span></div>
    ${d.files.map(f => `<div class="row"><span class="p">${f[0]}</span><span class="add">+${f[1]}</span><span class="del">-${f[2]}</span><span class="bar">${bar(f[1], f[2])}</span></div>`).join('')}
    <div class="df">${d.merge ? `<button class="btn sm pri">${I('merge', 12)} Merge</button>` : ''}<button class="btn sm">Review diff</button><button class="btn sm ghost">${I('branch', 12)} Open worktree</button></div></div>`;
}
function items(tid, aid) {
  const t = task(tid);
  return (CHATS[`${tid}/${aid}`] || []).map(it => {
    if (it.t === 'handoff') return `<div class="m-user"><div class="from">${it.from ? `from ${it.from}` : 'Home handed this off'}</div>${it.text}</div>`;
    if (it.t === 'text') { const a = AG(t, it.who); return `<div><div class="who">${avatar(t, it.who)}<b>${a.name}</b>${t.machine ? `<span class="machine">${I('server')}${t.machine}</span>` : ''}</div><div class="m-text">${it.html}</div></div>`; }
    if (it.t === 'tools') return `<div class="tools">${it.rows.map(toolRow).join('')}</div>`;
    if (it.t === 'event') return `<div class="m-event">${it.text}</div>`;
    if (it.t === 'diff') return diffCard(it);
    return '';
  }).join('');
}
function pending(tid, aid, compact) {
  const p = PENDING[`${tid}/${aid}`]; if (!p) return '';
  const who = AG(task(tid), aid).name;
  if (p.kind === 'question') return `<div class="ask-card"><div class="ah">${I('shield', 15)}${who} asks<span class="ag">T${task(tid).n}</span></div><div class="qh">${p.header}</div><div class="q">${p.q}</div>
    <div class="opts">${p.opts.map((o, i) => `<button class="opt${i === 0 ? ' on' : ''}" onclick="pickOpt(this)">${o[0]}${compact ? '' : `<small>${o[1]}</small>`}</button>`).join('')}</div>
    <div class="acts"><button class="btn pri">Answer</button><span class="hint">or type an answer below</span></div></div>`;
  return `<div class="ask-card"><div class="ah">${I('shield', 15)}${who} wants to run ${p.tool}<span class="ag">T${task(tid).n}</span></div><div class="cmd">${p.cmd}</div>${compact ? '' : `<div class="subtle" style="font:11.5px var(--mono)">${p.detail}</div>`}
    <div class="acts"><button class="btn pri">Allow <span class="k">Y</span></button><button class="btn">Always allow <span class="k">A</span></button><button class="btn">Deny <span class="k">N</span></button></div></div>`;
}
function composer(label, opts = {}) {
  return `<div class="composer"${opts.style ? ` style="${opts.style}"` : ''}><div class="composer-box"><div class="ph">${opts.ph || `Message ${label}…`}</div><div class="row">
    <span class="to">to <b>${label}</b></span>${opts.chips || ''}<span class="grow"></span><span class="chip">${opts.model || 'Sonnet 4.6'}</span><span class="iconbtn">${I('mic')}</span><span class="send">${I('send')}</span></div></div></div>`;
}
function agentChips(t, active, onclick) {
  if (t.mode !== 'fork') return '';
  return `<div class="agents">${t.agents.map(a => `<button class="agent${a.id === active ? ' on' : ''}" onclick="${onclick}('${t.id}','${a.id}')"><span class="sdot sd-${a.status}"></span>${a.name}<span class="r">${a.role}</span></button>`).join('')}</div>`;
}
function paneMenu(t, aid, id) {
  const a = AG(t, aid);
  return `<span class="menu-wrap"><button class="iconbtn" title="More" onclick="toggleMenu('${id}', event)">${I('more')}</button><div class="menu" id="${id}">
    <button class="mi">${I('ext')} Open ${a.name}'s pane<span class="k">${a.pane}</span></button>
    <button class="mi">${I('branch')} Open worktree<span class="k">${a.branch}</span></button>
    ${t.mode === 'fork' ? `<button class="mi">${I('split')} Show all ${t.agents.length} panes</button>` : ''}
    <div class="sep"></div><button class="mi" style="color:var(--danger)">${I('x')} Close task (agents stay)</button></div></span>`;
}
function card(t, on, attrs = '') {
  const meta = `${tile(t.ws)}<span>${t.ws}</span>·<span>${HOW[t.mode]}</span>·<b>${t.model}</b>`;
  const ag = t.mode === 'fork' ? `<div class="meta">${t.agents.map(a => `<span style="display:inline-flex;align-items:center;gap:4px"><span class="sdot sd-${a.status}"></span>${a.name}</span>`).join('')}</div>` : '';
  return `<div class="card${on ? ' on' : ''}${t.stage === 'merged' ? ' done' : ''}" ${attrs}><div class="h"><span class="id">T${t.n}</span>${pill(t.stage)}${t.machine ? `<span class="machine">${I('server')}${t.machine}</span>` : ''}<span class="grow"></span></div>
    <div class="goal">${t.goal}</div>${t.ask ? `<div class="ask">${I('shield', 12)}${t.ask}</div>` : ''}${t.step ? `<div class="stp"><i></i>${t.step}</div>` : ''}${ag}<div class="meta">${meta}</div></div>`;
}
const HOME_CHAT = `
  <div class="m-user">Add CSV export to the reports page in web, and fix that flaky login test in api while you're at it.</div>
  <div><div class="who"><span class="avatar" style="background:#7c8cff">${'H'}</span><b>Home</b></div><div class="m-text"><p>Two tasks: CSV export is bigger, so I forked it with its own orchestrator (<a href="#">T2</a>). The login flake is a one-agent fix (<a href="#">T1</a>).</p></div></div>
  <div class="tools">${toolRow({ n: 'update_task', ti: 'T2 · fork · web · Opus 4.6', tm: '1s', ok: 1 })}${toolRow({ n: 'update_task', ti: 'T1 · light · api · Sonnet 4.6', tm: '1s', ok: 1 })}</div>
  <div class="m-user">Also bump tokio on the hetzner box and start the 0.42 release on the studio.</div>
  <div><div class="who"><span class="avatar" style="background:#7c8cff">H</span><b>Home</b></div><div class="m-text"><p>Started on the other machines: <a href="#">T3</a> on <span class="machine">${I('server')}hetzner-01</span> and <a href="#">T4</a> on <span class="machine">${I('server')}mac-studio</span>. T3 is ready to merge; T2 needs you twice.</p></div></div>`;

function sidebar() {
  return `<div class="sidebar"><div class="nav on">${I('home')}Home<span class="dot"></span></div><div class="nav">${I('search')}Search</div>
    <div class="side-h">Workspaces</div>
    ${['api', 'web', 'core', 'winfer'].map((w, i) => `<div class="ws">${tile(w)}${w}<span class="n">${[2, 5, 1, 3][i]}</span></div>`).join('')}
    <div class="side-h">Machines</div><div class="ws">${I('server')}hetzner-01<span class="n">online</span></div><div class="ws">${I('server')}mac-studio<span class="n">online</span></div></div>`;
}
function windowChrome(inner) {
  return `<div class="window"><div class="titlebar"><div class="lights"><i></i><i></i><i></i></div><div class="t">winfer · Home</div><div style="width:46px"></div></div><div class="body">${sidebar()}${inner}</div></div>`;
}
function pickOpt(b) { b.parentElement.querySelectorAll('.opt').forEach(o => o.classList.remove('on')); b.classList.add('on'); }
function toggleMenu(id, e) { e.stopPropagation(); document.querySelectorAll('.menu.open').forEach(m => m.id !== id && m.classList.remove('open')); document.getElementById(id).classList.toggle('open'); }
document.addEventListener('click', () => document.querySelectorAll('.menu.open').forEach(m => m.classList.remove('open')));
function settle() { document.querySelectorAll('.window .chat').forEach(c => c.scrollTop = c.scrollHeight); }
