// Mockup chrome + tiny state binding. Each page calls mockup({ id, pitch, state }) and marks controls up with:
//   data-bind="key"  on .toggle / .seg / select / input / .chip groups (state[key])
//   data-if="key" | "!key" | "key=val" | "key=a,b" | "k1&k2"   shown only when true
//   data-text="key"  shows state[key];  page code can add listeners via onChange(fn).
const MOCKUPS = [
  ["a-tabs.html", "A · Tabs"],
  ["b-overview.html", "B · Overview + sections"],
  ["c-visual.html", "C · Visual controls"],
  ["d-visual-tabs.html", "D · Visual + tabs"],
];
const NAV = [
  ["Workspace", [["sliders-horizontal", "General"], ["palette", "Appearance"]]],
  ["Agents", [["bot", "Agents"], ["square-terminal", "winfer code"]]],
  ["AI & voice", [["users", "Accounts"], ["sparkles", "AI"], ["mic", "Voice"]]],
  ["Extensions", [["puzzle", "Plugins"], ["wand-2", "Skills"]]],
  ["System", [["chart-column", "Usage & limits"], ["smartphone", "Mobile"], ["archive-restore", "Backup & transfer"], ["download", "Updates"], ["activity", "Diagnostics"]]],
];
const state = {};
const listeners = [];
const onChange = (fn) => listeners.push(fn);
const set = (patch) => {
  Object.assign(state, patch);
  refresh();
};

function mockup({ id, pitch, initial }) {
  Object.assign(state, initial);
  if (localStorage.getItem("mock.theme") === "light") document.documentElement.classList.add("light");
  const bar = document.createElement("div");
  bar.className = "reviewbar";
  bar.innerHTML =
    MOCKUPS.map(([f, l]) => `<a href="${f}" class="${f.startsWith(id) ? "on" : ""}">${l}</a>`).join("") +
    `<span class="sep"></span><button class="theme" id="themeBtn"><i data-lucide="sun-moon"></i>Light / dark</button>`;
  const p = document.createElement("div");
  p.className = "pitch";
  p.innerHTML = pitch;
  document.body.prepend(bar, p);
  document.getElementById("themeBtn").onclick = () => {
    const light = document.documentElement.classList.toggle("light");
    localStorage.setItem("mock.theme", light ? "light" : "dark");
  };
  document.querySelector("nav.side").innerHTML =
    `<h1><i data-lucide="settings-2"></i>Settings</h1><div class="search"><i data-lucide="search"></i>Search settings</div><div class="navlist">` +
    NAV.map(([g, items]) => `<div class="navgroup">${g}</div>` + items.map(([ic, l]) => `<button class="navitem ${l === "winfer code" ? "on" : ""}"><i data-lucide="${ic}"></i>${l}</button>`).join("")).join("") +
    `</div><div class="navfoot">winfer 0.42.0</div>`;
  wire(document);
  refresh();
  lucide.createIcons();
  // Real size (1000×700, like the app); scaled down only when the window is narrower.
  const fit = () => (document.querySelector(".dialog").style.zoom = Math.min(1, (innerWidth - 24) / 1000));
  addEventListener("resize", fit);
  fit();
}

function wire(root) {
  root.querySelectorAll(".toggle[data-bind]").forEach((t) => {
    t.setAttribute("role", "switch");
    t.onclick = () => set({ [t.dataset.bind]: !state[t.dataset.bind] });
  });
  root.querySelectorAll(".seg[data-bind], [data-radio]").forEach((s) => {
    const key = s.dataset.bind ?? s.dataset.radio;
    s.querySelectorAll("[data-v]").forEach((b) => {
      b.setAttribute("role", "radio");
      b.onclick = () => set({ [key]: b.dataset.v });
    });
  });
  root.querySelectorAll("select[data-bind], input[data-bind]").forEach((el) => {
    const ev = el.type === "range" ? "input" : "change";
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
    s.querySelectorAll("[data-v]").forEach((b) => b.setAttribute("aria-checked", b.dataset.v === String(state[key])));
  });
  document.querySelectorAll("select[data-bind], input[data-bind]").forEach((el) => {
    if (document.activeElement !== el && state[el.dataset.bind] !== undefined) el.value = state[el.dataset.bind];
  });
  document.querySelectorAll("[data-chips]").forEach((g) => g.querySelectorAll(".chip").forEach((c) => c.setAttribute("aria-pressed", state[g.dataset.chips].includes(c.dataset.v))));
  document.querySelectorAll("[data-if]").forEach((el) => (el.hidden = !test(el.dataset.if)));
  document.querySelectorAll("[data-off-if]").forEach((el) => el.classList.toggle("is-off", test(el.dataset.offIf)));
  document.querySelectorAll("[data-text]").forEach((el) => (el.textContent = state[el.dataset.text]));
  listeners.forEach((fn) => fn(state));
}

// The real page's data: one place so the three mockups show the same settings.
const DEFAULTS = {
  ui: "web", showContext: true, showCost: false, showRate: true, showPlan: true, expandThinking: false, expandShell: true, compactTools: true, compact: false, textScale: 100, planSidebar: true,
  model: "", effort: "", mode: "ask", autoApprove: false, allowed: "git, cargo, pnpm",
  subChatgpt: "gpt-6-luna-mini", subChatgptEffort: "low", subClaude: "", subCodex: "", subLocal: "",
  local: true, host: "127.0.0.1", port: "8000", path: "/v1", apiKey: "", ctx: "", levels: "none, low, medium, xhigh", templateEffort: true, webSearch: true, budgetLow: "1024", budgetMedium: "4096", budgetXhigh: "",
  leanClaude: true, leanCodex: true, shortCodex: false, leanCodexCli: false, skillsMode: "selected", skills: ["frontend-design", "dataviz", "pdf"],
  trimAt: 60, compactAt: 85,
};
const SKILLS = ["frontend-design", "dataviz", "pdf", "docx", "xlsx", "pptx", "skill-creator", "webapp-testing", "superpowers:brainstorm", "superpowers:tdd"];
const MODEL_OPTIONS = `
  <option value="">Automatic</option>
  <optgroup label="ChatGPT account"><option>gpt-6-luna</option><option>gpt-6-luna-mini</option><option>gpt-6.1-sol</option></optgroup>
  <optgroup label="Claude Code"><option>claude-opus-5-5</option><option>claude-sonnet-5</option><option>claude-haiku-5</option></optgroup>
  <optgroup label="Codex"><option>gpt-6.1-sol-codex</option><option>gpt-6-luna-codex</option></optgroup>
  <optgroup label="Local model"><option>local/qwen3.8-coder-32b</option><option>local/devstral-3</option></optgroup>`;
const EFFORT_OPTIONS = `<option value="">Model default</option><option>minimal</option><option>low</option><option>medium</option><option>high</option><option>xhigh</option><option>max</option>`;
