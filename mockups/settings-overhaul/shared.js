// Settings overhaul mockups: one page per file, the sidebar links them. Same tiny state binding as
// the winfer-code mockups:
//   data-bind="key"  on .toggle / .seg / select / input / .chip groups (state[key])
//   data-radio="key" on any group of [data-v] buttons (tabs, cards)
//   data-if="key" | "!key" | "key=val" | "key=a,b" | "k1&k2"   shown only when true
//   data-text="key"  shows state[key];  page code can add listeners via onChange(fn).
const NAV = [
  ["Workspace", [["general", "sliders-horizontal", "General"], ["theme", "palette", "Appearance"]]],
  ["Agents", [["agents", "bot", "Agents"], ["winfer-code", "square-terminal", "WinferCode"]]],
  ["AI & voice", [["accounts", "users", "Accounts"], ["ai", "sparkles", "AI"], ["voice", "mic", "Voice"]]],
  ["Extensions", [["plugins", "puzzle", "Plugins"], ["skills", "wand-sparkles", "Skills"]]],
  ["System", [["usage", "chart-column", "Usage & limits"], ["mobile", "smartphone", "Mobile"], ["data", "archive-restore", "Backup & transfer"], ["updates", "download", "Updates"], ["diagnostics", "activity", "Diagnostics"]]],
];
const href = (id) => (id === "winfer-code" ? "../winfer-code-settings/d-visual-tabs.html" : `${id}.html`);
const PAGES = NAV.flatMap(([, items]) => items.map(([id]) => id));
const state = {};
const listeners = [];
const onChange = (fn) => listeners.push(fn);
const set = (patch) => {
  Object.assign(state, patch);
  refresh();
};
const $ = (id) => document.getElementById(id);

function mockup({ id, pitch, initial = {}, tabs }) {
  Object.assign(state, initial);
  if (tabs) state.tab = tabs.includes(location.hash.slice(1)) ? location.hash.slice(1) : tabs[0];
  if (localStorage.getItem("mock.theme") === "light") document.documentElement.classList.add("light");
  const i = PAGES.indexOf(id);
  const prev = PAGES[(i + PAGES.length - 1) % PAGES.length];
  const next = PAGES[(i + 1) % PAGES.length];
  const name = (p) => NAV.flatMap(([, it]) => it).find((x) => x[0] === p)[2];
  const bar = document.createElement("div");
  bar.className = "reviewbar";
  bar.innerHTML =
    `<a href="${href(prev)}"><i data-lucide="chevron-left"></i>${name(prev)}</a><span class="where">${i + 1} / ${PAGES.length} · ${name(id)}</span><a href="${href(next)}">${name(next)}<i data-lucide="chevron-right"></i></a>` +
    `<span class="sep"></span><a href="index.html"><i data-lucide="layout-grid"></i>All pages</a><button class="theme" id="themeBtn"><i data-lucide="sun-moon"></i>Light / dark</button>`;
  const p = document.createElement("div");
  p.className = "pitch";
  p.innerHTML = pitch;
  document.body.prepend(bar, p);
  $("themeBtn").onclick = () => {
    const light = document.documentElement.classList.toggle("light");
    localStorage.setItem("mock.theme", light ? "light" : "dark");
  };
  document.querySelector("nav.side").innerHTML =
    `<h1><i data-lucide="settings-2"></i>Settings</h1><div class="search"><i data-lucide="search"></i>Search settings</div><div class="navlist">` +
    NAV.map(([g, items]) => `<div class="navgroup">${g}</div>` + items.map(([pid, ic, l]) => `<a class="navitem ${pid === id ? "on" : ""}" href="${href(pid)}"><i data-lucide="${ic}"></i>${l}</a>`).join("")).join("") +
    `</div><div class="navfoot">winfer 0.42.0</div>`;
  wire(document);
  refresh();
  lucide.createIcons();
  const fit = () => (document.querySelector(".dialog").style.zoom = Math.min(1, (innerWidth - 24) / 1000));
  addEventListener("resize", fit);
  fit();
}

function wire(root) {
  root.querySelectorAll(".toggle[data-bind]").forEach((t) => {
    t.setAttribute("role", "switch");
    t.onclick = (e) => {
      e.stopPropagation();
      set({ [t.dataset.bind]: !state[t.dataset.bind] });
    };
  });
  root.querySelectorAll(".seg[data-bind], [data-radio]").forEach((s) => {
    const key = s.dataset.bind ?? s.dataset.radio;
    s.querySelectorAll("[data-v]").forEach((b) => {
      if (b.closest("[data-radio], .seg[data-bind]") !== s) return;
      b.setAttribute("role", "radio");
      b.onclick = () => set({ [key]: b.dataset.v });
    });
  });
  root.querySelectorAll("select[data-bind], input[data-bind]").forEach((el) => {
    const ev = el.type === "range" || el.type === "text" || el.type === "search" ? "input" : "change";
    el.addEventListener(ev, () => set({ [el.dataset.bind]: el.type === "range" || el.dataset.num ? Number(el.value) : el.value }));
  });
  root.querySelectorAll("[data-chips]").forEach((g) => {
    g.querySelectorAll(".chip").forEach((c) => {
      c.onclick = () => {
        const list = new Set(state[g.dataset.chips]);
        list.has(c.dataset.v) ? list.delete(c.dataset.v) : list.add(c.dataset.v);
        set({ [g.dataset.chips]: [...list] });
      };
    });
  });
}

const test = (cond) =>
  cond.split("&").every((c) => {
    if (c.startsWith("!")) return !state[c.slice(1)];
    const [k, v] = c.split("=");
    return v === undefined ? !!state[k] : v.split(",").includes(String(state[k]));
  });

function refresh() {
  document.querySelectorAll(".toggle[data-bind]").forEach((t) => t.setAttribute("aria-checked", !!state[t.dataset.bind]));
  document.querySelectorAll(".seg[data-bind], [data-radio]").forEach((s) => {
    const key = s.dataset.bind ?? s.dataset.radio;
    s.querySelectorAll("[data-v]").forEach((b) => {
      if (b.closest("[data-radio], .seg[data-bind]") === s) b.setAttribute("aria-checked", b.dataset.v === String(state[key]));
    });
  });
  document.querySelectorAll("select[data-bind], input[data-bind]").forEach((el) => {
    if (document.activeElement !== el && state[el.dataset.bind] !== undefined) el.value = state[el.dataset.bind];
  });
  document.querySelectorAll("[data-chips]").forEach((g) => g.querySelectorAll(".chip").forEach((c) => c.setAttribute("aria-pressed", state[g.dataset.chips].includes(c.dataset.v))));
  document.querySelectorAll("[data-if]").forEach((el) => (el.hidden = !test(el.dataset.if)));
  document.querySelectorAll("[data-off-if]").forEach((el) => el.classList.toggle("is-off", test(el.dataset.offIf)));
  document.querySelectorAll("[data-text]").forEach((el) => (el.textContent = state[el.dataset.text]));
  listeners.forEach((fn) => fn(state));
  if (state.tab && location.hash.slice(1) !== state.tab) history.replaceState(null, "", "#" + state.tab);
}

// Agent brand marks used across pages.
const AGENTS = {
  claude: { name: "Claude Code", color: "#d97757", letter: "C" },
  codex: { name: "Codex", color: "#10a37f", letter: "X" },
  agy: { name: "Antigravity", color: "#4f8cff", letter: "A" },
  opencode: { name: "opencode", color: "#f5b544", letter: "O" },
  cursor: { name: "Cursor", color: "#a0a0ff", letter: "▲" },
  grok: { name: "Grok", color: "#d4d4d8", letter: "G" },
  pi: { name: "pi", color: "#7ee787", letter: "π" },
  muse: { name: "Muse", color: "#fb7185", letter: "M" },
  winfer: { name: "WinferCode", color: "#7c8cff", letter: "W" },
};
const mark = (a, size = 26) => {
  const g = AGENTS[a];
  return `<span class="amark" style="--c:${g.color};width:${size}px;height:${size}px;font-size:${Math.round(size * 0.42)}px">${g.letter}</span>`;
};
