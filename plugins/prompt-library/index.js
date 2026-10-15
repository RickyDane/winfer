// Prompt Library: prompts the user keeps reusing, with {{placeholders}} filled in when sent.
// They're sent to the focused agent, broadcast to every agent or used to start a new one, from
// the page, the ⌘K palette ("Prompt: …") or by voice ("send the review prompt to all agents").
// Agents can list, read and (with the user's OK) save prompts through tools.
//
// Placeholders: {{name}} asks for a value, {{name:default}} suggests one, and a few fill
// themselves: {{workspace}}, {{project}}, {{pane}}, {{date}}, {{time}} and {{clipboard}}.

const STORAGE = "winfer.prompt-library";

export const BUILTINS = {
  workspace: "The workspace's name",
  project: "The project folder",
  pane: "The agent it's sent to",
  date: "Today's date",
  time: "The time now",
  clipboard: "What's on the clipboard",
};

const TARGETS = {
  focused: "the focused agent",
  all: "every agent in the workspace",
  new: "a new agent",
};

const STARTERS = [
  {
    title: "Review my changes",
    tags: ["review"],
    body: "Review the uncommitted changes in this repo (git diff). Point out bugs, risky edge cases and anything that doesn't match the surrounding code. For each: file, line, problem, fix.",
  },
  {
    title: "Write tests",
    tags: ["tests"],
    body: "Write tests for {{target}}. Cover the main paths and the edge cases ({{cases:empty input, errors, boundaries}}). Follow the project's existing test style, then run them.",
  },
  {
    title: "Fix failing tests",
    tags: ["tests"],
    body: "Run the test suite, find out why the failing tests fail and fix the cause, not the tests (unless they're wrong). Rerun until everything passes, then summarize what was broken.",
  },
  {
    title: "Explain how it works",
    tags: ["explain"],
    body: "Explain how {{topic}} works in this codebase: the entry points, how data flows and which files are involved, with file:line references. Keep it short.",
  },
  {
    title: "Plan first",
    tags: ["planning"],
    body: "Before writing any code, make a plan for:\n\n{{task}}\n\nList the files you'll change, the approach and the risks. Then wait for my OK.",
  },
  {
    title: "Commit message",
    tags: ["git"],
    body: "Write a commit message for the staged changes: a subject under 60 characters, a blank line, then a body explaining why. Don't commit.",
  },
];

const VAR = /\{\{\s*([A-Za-z_][\w.-]*)\s*(?::([^}]*))?\}\}/g;

/** The placeholders of a prompt the user fills in, in order: `[{ name, def }]` (built-ins left out). */
export function parseVars(body) {
  const seen = new Map();
  for (const m of String(body).matchAll(VAR)) {
    const name = m[1];
    if (name in BUILTINS) continue;
    const def = m[2]?.trim();
    if (!seen.has(name)) seen.set(name, { name, def: def ?? "" });
    else if (!seen.get(name).def && def) seen.get(name).def = def;
  }
  return [...seen.values()];
}

/** The built-in placeholders a prompt uses. */
export const usedBuiltins = (body) => [...new Set([...String(body).matchAll(VAR)].map((m) => m[1]).filter((n) => n in BUILTINS))];

/** Fills the placeholders: `values` first, then the placeholder's default, else empty. */
export function renderPrompt(body, values = {}) {
  // A default given once counts for every use of the placeholder.
  const defaults = Object.fromEntries(parseVars(body).map((v) => [v.name, v.def]));
  return String(body).replace(VAR, (_, name, def) => {
    const v = values[name];
    return v !== undefined && v !== null && String(v) !== "" ? String(v) : (def?.trim() || defaults[name] || "");
  });
}

/** Prompts from an import (an array or `{ prompts }`), checked and tidied. */
export function normalizePrompts(data) {
  const list = Array.isArray(data) ? data : Array.isArray(data?.prompts) ? data.prompts : null;
  if (!list) throw new Error("Expected a list of prompts, or { \"prompts\": [...] }.");
  return list.map((p, i) => {
    if (!p || typeof p.title !== "string" || !p.title.trim() || typeof p.body !== "string") throw new Error(`Prompt ${i + 1} needs a title and a body.`);
    return {
      id: typeof p.id === "string" && p.id ? p.id : newId(),
      title: p.title.trim(),
      body: p.body,
      tags: Array.isArray(p.tags) ? [...new Set(p.tags.map((t) => String(t).trim().toLowerCase()).filter(Boolean))] : [],
      uses: Number(p.uses) || 0,
      lastUsed: Number(p.lastUsed) || 0,
      createdAt: Number(p.createdAt) || Date.now(),
      updatedAt: Number(p.updatedAt) || Date.now(),
    };
  });
}

/** Finds a prompt by id or title (exact, then the start of a title, ignoring case). */
export function findPrompt(prompts, name) {
  const wanted = String(name ?? "").trim().toLowerCase();
  if (!wanted) return null;
  return prompts.find((p) => p.id === name) ?? prompts.find((p) => p.title.toLowerCase() === wanted) ?? prompts.find((p) => p.title.toLowerCase().startsWith(wanted)) ?? null;
}

function newId() {
  return `p${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

const parseTags = (text) => [...new Set(String(text).split(",").map((t) => t.trim().toLowerCase()).filter(Boolean))];

function ago(ms) {
  if (!ms) return "never";
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86_400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86_400)} d ago`;
}

// Store -----------------------------------------------------------------------------------------

function createStore() {
  let prompts = [];
  let saved = null;
  try {
    saved = JSON.parse(localStorage.getItem(STORAGE) ?? "null");
  } catch {
    saved = null;
  }
  try {
    prompts = saved ? normalizePrompts(saved) : normalizePrompts(STARTERS);
  } catch {
    prompts = normalizePrompts(STARTERS);
  }
  const listeners = new Set();
  const changed = () => {
    try {
      localStorage.setItem(STORAGE, JSON.stringify({ prompts }));
    } catch {
      // Storage full or off: changes last until winfer restarts.
    }
    listeners.forEach((fn) => fn());
  };
  if (!saved) changed();

  return {
    all: () => prompts,
    get: (id) => prompts.find((p) => p.id === id) ?? null,
    add(fields) {
      const now = Date.now();
      const p = { id: newId(), title: "Untitled prompt", body: "", tags: [], uses: 0, lastUsed: 0, createdAt: now, updatedAt: now, ...fields };
      prompts = [p, ...prompts];
      changed();
      return p;
    },
    update(id, fields) {
      prompts = prompts.map((p) => (p.id === id ? { ...p, ...fields, updatedAt: Date.now() } : p));
      changed();
    },
    remove(id) {
      prompts = prompts.filter((p) => p.id !== id);
      changed();
    },
    used(id) {
      prompts = prompts.map((p) => (p.id === id ? { ...p, uses: p.uses + 1, lastUsed: Date.now() } : p));
      changed();
    },
    /** Adds imported prompts; one with the same id replaces the old one. */
    merge(incoming) {
      const ids = new Set(incoming.map((p) => p.id));
      prompts = [...incoming, ...prompts.filter((p) => !ids.has(p.id))];
      changed();
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

// Plugin ----------------------------------------------------------------------------------------

export function activate(api) {
  const store = createStore();
  const style = h("style", {}, CSS);
  document.head.append(style);

  /** The workspace, project and pane a prompt is sent from/to, for the built-in placeholders. */
  function context(target) {
    const s = api.state();
    const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId);
    const paneId = typeof target === "object" ? target.paneId : target === "focused" ? s.focusedPaneId : null;
    const pane = paneId ? api.describePane(paneId) : null;
    return { workspace: ws?.name ?? "", project: pane?.cwd ?? ws?.rootDir ?? "", pane: pane?.label ?? "" };
  }

  async function builtinValues(body, target) {
    const used = usedBuiltins(body);
    const ctx = context(target);
    const now = new Date();
    const values = { ...ctx, date: now.toLocaleDateString(), time: now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) };
    if (used.includes("clipboard")) {
      try {
        values.clipboard = await navigator.clipboard.readText();
      } catch {
        values.clipboard = "";
      }
    }
    return values;
  }

  /** Agents of the active workspace the user can send to. */
  function panes() {
    const s = api.state();
    const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId);
    return (ws?.panes ?? []).map((p) => api.describePane(p.id)).filter(Boolean);
  }

  /** Sends a filled-in prompt; `target` is "focused", "all", "new" or `{ paneId }`. */
  async function send(prompt, values, target, submit = true) {
    const text = renderPrompt(prompt.body, { ...(await builtinValues(prompt.body, target)), ...values }).trim();
    if (!text) {
      api.toast("The prompt is empty.", "error");
      return;
    }
    const s = api.state();
    if (target === "focused" && !s.focusedPaneId) {
      api.toast("No agent is focused. Pick one, or send to a new agent.", "error");
      return;
    }
    if (target === "all") api.execute({ type: "pane.broadcast", text, submit });
    else if (target === "new") api.execute({ type: "pane.spawn", provider: "default", prompt: text });
    else {
      const ref = typeof target === "object" ? { id: target.paneId } : "focused";
      api.execute({ type: "pane.sendText", target: ref, text, submit });
      api.execute({ type: "pane.focus", target: ref });
    }
    api.execute({ type: "ui.view", view: "workspace" });
    store.used(prompt.id);
    const where = typeof target === "object" ? (api.describePane(target.paneId)?.label ?? "the agent") : TARGETS[target];
    api.toast(`Sent “${prompt.title}” to ${where}`, "success");
  }

  /** Sends right away when nothing needs filling in; else asks for the placeholders first. */
  function use(prompt, target = "focused") {
    if (parseVars(prompt.body).length === 0) return send(prompt, {}, target);
    openFill(prompt, target);
  }

  function openFill(prompt, initialTarget = "focused") {
    document.querySelector(".pl-fill-backdrop")?.remove();
    const vars = parseVars(prompt.body);
    const inputs = new Map();
    const preview = h("div", { class: "pl-preview" });
    const values = () => Object.fromEntries([...inputs].map(([name, el]) => [name, el.value]));
    const refresh = async () => {
      const filled = renderPrompt(prompt.body, { ...(await builtinValues(prompt.body, targetValue())), ...values() });
      preview.textContent = filled.trim() || "(empty)";
    };
    const fields = vars.map((v, i) => {
      const multi = /task|description|details|context|notes|text|body/i.test(v.name);
      const el = h(multi ? "textarea" : "input", {
        class: "pl-text",
        value: v.def,
        rows: multi ? 3 : undefined,
        placeholder: v.def || `Value for ${v.name}`,
        spellcheck: "false",
        oninput: refresh,
        autofocus: i === 0,
      });
      if (multi) el.value = v.def;
      inputs.set(v.name, el);
      return h("label", { class: "pl-field" }, h("span", { class: "pl-mono" }, v.name), el);
    });
    const targetSelect = targetPicker(initialTarget);
    const targetValue = () => (targetSelect.value.startsWith("pane:") ? { paneId: targetSelect.value.slice(5) } : targetSelect.value);
    targetSelect.addEventListener("change", refresh);
    const submitBox = h("input", { type: "checkbox", class: "wf-switch", checked: true });
    const close = () => backdrop.remove();
    const go = async (e) => {
      e.preventDefault();
      close();
      await send(prompt, values(), targetValue(), submitBox.checked);
    };
    const backdrop = h(
      "div",
      { class: "wf-dialog-backdrop pl-fill-backdrop", onmousedown: (e) => e.target === backdrop && close() },
      h(
        "form",
        {
          class: "wf-dialog pl-dialog",
          onsubmit: go,
          onkeydown: (e) => {
            if (e.key === "Escape") (e.stopPropagation(), close());
            else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) go(e);
          },
        },
        h("div", { class: "pl-dialog-head" }, h("div", { class: "pl-logo" }, icon("text")), h("div", { class: "pl-dialog-title" }, h("h2", { class: "wf-h2", style: "font-weight:600" }, prompt.title), h("div", { class: "wf-subtle" }, "Fill in the blanks, then send."))),
        h("div", { class: "pl-dialog-body" }, fields, h("div", { class: "pl-field" }, h("span", {}, "Preview"), preview)),
        h(
          "div",
          { class: "pl-dialog-foot" },
          targetSelect,
          h("label", { class: "pl-submit", title: "Press Enter after typing it in" }, submitBox, "Submit"),
          h("span", { class: "pl-grow" }),
          h("button", { class: "wf-button wf-ghost", type: "button", onclick: close }, "Cancel"),
          h("button", { class: "wf-button wf-primary", type: "submit" }, icon("send"), "Send", h("span", { class: "pl-kbd" }, "⌘↵")),
        ),
      ),
    );
    document.body.append(backdrop);
    void refresh();
    (inputs.values().next().value ?? targetSelect).focus();
  }

  /** A select of where to send: focused agent, each agent of the workspace, all, or a new one. */
  function targetPicker(initial = "focused") {
    const focusedLabel = api.describePane(api.state().focusedPaneId ?? "")?.label;
    const select = h(
      "select",
      { class: "pl-select", title: "Send to" },
      h("option", { value: "focused" }, focusedLabel ? `Focused: ${focusedLabel}` : "Focused agent"),
      panes().map((p) => h("option", { value: `pane:${p.paneId}` }, p.label)),
      h("option", { value: "all" }, "All agents"),
      h("option", { value: "new" }, "New agent"),
    );
    select.value = typeof initial === "object" ? `pane:${initial.paneId}` : initial;
    // The agent it pointed at may be gone.
    if (!select.value) select.value = "focused";
    return select;
  }

  /** A prompt made from the palette, for the page to open on. */
  let pendingSelect = null;

  // Palette and voice: one tool, with a palette entry per prompt; re-registered when they change.
  let unregister = [];
  function syncTools() {
    unregister.forEach((d) => d());
    unregister = [];
    const prompts = [...store.all()].sort((a, b) => b.uses - a.uses || a.title.localeCompare(b.title));
    if (prompts.length > 0) {
      unregister.push(
        api.registerTool({
          id: "use",
          title: "Use a saved prompt",
          description: "Send one of the user's saved prompts from the prompt library to an agent, to all agents or to a new agent",
          params: {
            prompt: { kind: "enum", ask: "Which saved prompt?", options: Object.fromEntries(prompts.map((p) => [p.id, p.title])) },
            target: { kind: "enum", ask: "Send it to which agent?", options: TARGETS },
          },
          palette: prompts.map((p) => ({ title: `Prompt: ${p.title}`, args: { prompt: p.id, target: "focused" } })),
          run: (args) => {
            const prompt = store.get(String(args.prompt ?? "")) ?? findPrompt(store.all(), args.prompt);
            if (!prompt) return api.toast("That prompt isn't in the library anymore.", "error");
            return use(prompt, TARGETS[args.target] ? args.target : "focused");
          },
        }),
      );
    }
  }
  syncTools();
  // Edits arrive per keystroke; the palette only needs the result.
  let syncTimer = null;
  const unsubscribe = store.subscribe(() => {
    clearTimeout(syncTimer);
    syncTimer = setTimeout(syncTools, 400);
  });

  api.registerTool({
    id: "open",
    title: "Prompt Library",
    description: "Open the prompt library to write, edit and organize saved prompts",
    palette: [{ title: "Prompt Library: open" }],
    run: () => api.openPage("prompts"),
  });
  api.registerTool({
    id: "new",
    title: "New saved prompt",
    description: "Write a new prompt in the prompt library",
    palette: [{ title: "Prompt Library: new prompt" }],
    run: () => {
      pendingSelect = store.add({}).id;
      api.openPage("prompts");
    },
  });

  api.registerAgentTool({
    name: "prompt_list",
    description: "List the prompts the user saved in winfer's Prompt Library (titles, tags and the {{placeholders}} each one takes). Read one with prompt_get.",
    inputSchema: { type: "object", properties: { tag: { type: "string", description: "Only prompts with this tag" } } },
    run(args) {
      const tag = args.tag ? String(args.tag).toLowerCase() : "";
      const list = store.all().filter((p) => !tag || p.tags.includes(tag));
      if (list.length === 0) return tag ? `No prompts tagged ${tag}.` : "The prompt library is empty.";
      return list
        .map((p) => {
          const vars = parseVars(p.body).map((v) => (v.def ? `${v.name} (default: ${v.def})` : v.name));
          return `${p.title} [${p.id}]${p.tags.length ? ` #${p.tags.join(" #")}` : ""}${vars.length ? ` — placeholders: ${vars.join(", ")}` : ""}`;
        })
        .join("\n");
    },
  });

  api.registerAgentTool({
    name: "prompt_get",
    description: "Get a saved prompt from the user's Prompt Library with its placeholders filled in, to follow it yourself or hand it to another agent.",
    inputSchema: {
      type: "object",
      properties: {
        name: { type: "string", description: "The prompt's title or id from prompt_list" },
        values: { type: "object", description: "Values for its placeholders, e.g. {\"target\": \"src/api.ts\"}", additionalProperties: { type: "string" } },
      },
      required: ["name"],
    },
    run(args, caller) {
      const prompt = findPrompt(store.all(), args.name);
      if (!prompt) throw new Error(`No prompt called “${args.name}”. Use prompt_list to see them.`);
      const given = args.values && typeof args.values === "object" ? args.values : {};
      const now = new Date();
      const builtins = { workspace: caller.workspaceName, project: caller.cwd, pane: caller.label, date: now.toLocaleDateString(), time: now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }), clipboard: "" };
      const missing = parseVars(prompt.body).filter((v) => !v.def && !given[v.name]).map((v) => v.name);
      const text = renderPrompt(prompt.body, { ...builtins, ...given });
      return missing.length ? `${text}\n\n(No value given for: ${missing.join(", ")}.)` : text;
    },
  });

  api.registerAgentTool({
    name: "prompt_save",
    description: "Save a new prompt to the user's Prompt Library (they approve it first). Use {{name}} for parts that change each time and {{name:default}} to suggest a value.",
    inputSchema: {
      type: "object",
      properties: {
        title: { type: "string", description: "Short title" },
        body: { type: "string", description: "The prompt text" },
        tags: { type: "array", items: { type: "string" } },
      },
      required: ["title", "body"],
    },
    async run(args, caller) {
      const title = String(args.title ?? "").trim();
      const body = String(args.body ?? "");
      if (!title || !body.trim()) throw new Error("A prompt needs a title and a body.");
      const ok = await api.confirm(`${caller.label} wants to save a prompt to your library:\n\n${title}\n\n${body}`, { title: "Save prompt?", okLabel: "Save", kind: "info" });
      if (!ok) throw new Error("The user declined saving this prompt.");
      const p = store.add({ title, body, tags: Array.isArray(args.tags) ? parseTags(args.tags.join(",")) : [] });
      return `Saved “${p.title}” [${p.id}].`;
    },
  });

  api.registerPage({
    id: "prompts",
    title: "Prompts",
    icon: "message-square-quote",
    mount: (el) => {
      const select = pendingSelect;
      pendingSelect = null;
      return mountPage(el, api, store, { use, openFill, targetPicker, send, select });
    },
  });

  return () => {
    unsubscribe();
    clearTimeout(syncTimer);
    unregister.forEach((d) => d());
    document.querySelector(".pl-fill-backdrop")?.remove();
    style.remove();
  };
}

// Page ------------------------------------------------------------------------------------------

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
  text: "M4 6h16M4 12h10M4 18h7M17 15l3 3-3 3",
  send: "M22 2 11 13M22 2l-7 20-4-9-9-4z",
  plus: "M12 5v14M5 12h14",
  copy: "M9 9h11v11H9zM5 15H4V4h11v1",
  trash: "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6",
  search: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3",
  download: "M12 3v12M7 10l5 5 5-5M5 21h14",
  upload: "M12 21V9M7 14l5-5 5 5M5 3h14",
  braces: "M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5a2 2 0 0 0 2 2h1M16 3h1a2 2 0 0 1 2 2v5a2 2 0 0 0 2 2 2 2 0 0 0-2 2v5a2 2 0 0 1-2 2h-1",
  bolt: "M13 2 3 14h9l-1 8 10-12h-9z",
  check: "M20 6 9 17l-5-5",
  alert: "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
};

function icon(name) {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("class", "pl-icon");
  for (const [k, v] of Object.entries({ fill: "none", stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round", "stroke-linejoin": "round" })) el.setAttribute(k, v);
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", ICONS[name]);
  el.append(path);
  return el;
}

const CSS = `
.pl-page { max-width: 1180px; gap: 16px; container-type: inline-size; }
.pl-icon { width: 14px; height: 14px; flex-shrink: 0; }
.pl-hero { display: flex; align-items: center; gap: 14px; flex-wrap: wrap; }
.pl-hero-text { flex: 1; min-width: 220px; display: flex; flex-direction: column; gap: 3px; }
.pl-logo { width: 40px; height: 40px; border-radius: 11px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; color: white; background: linear-gradient(160deg, #b48ae8, #7a55d6); box-shadow: inset 0 1px 0 rgb(255 255 255 / .25), 0 4px 14px rgb(122 85 214 / .25); }
.pl-logo .pl-icon { width: 20px; height: 20px; }
.pl-grid { display: grid; grid-template-columns: 300px minmax(0, 1fr); gap: 16px; align-items: start; }
@container (max-width: 780px) { .pl-grid { grid-template-columns: minmax(0, 1fr); } }
.pl-side, .pl-main { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
.pl-search { display: flex; align-items: center; gap: 8px; height: 30px; padding: 0 10px; border-radius: 8px; border: 1px solid var(--color-border-strong); background: var(--color-panel); }
.pl-search:focus-within { border-color: var(--color-accent); box-shadow: 0 0 0 3px var(--color-accent-soft); }
.pl-search > .pl-icon { color: var(--color-fg-subtle); }
.pl-search input { flex: 1; min-width: 0; height: 100%; border: 0; outline: none; background: transparent; color: var(--color-fg); font: inherit; font-size: 12px; user-select: text; }
.pl-chips { display: flex; gap: 4px; flex-wrap: wrap; }
.pl-chip { display: inline-flex; align-items: center; gap: 5px; height: 22px; padding: 0 8px; border-radius: 999px; border: 1px solid var(--color-border); background: transparent; font: inherit; font-size: 11px; color: var(--color-fg-muted); }
.pl-chip:hover { border-color: var(--color-border-strong); color: var(--color-fg); }
.pl-chip.pl-on { border-color: color-mix(in srgb, var(--color-accent) 50%, transparent); background: var(--color-accent-soft); color: var(--color-fg); }
.pl-list { max-height: 560px; overflow: auto; }
.pl-item { display: flex; align-items: flex-start; gap: 8px; padding: 9px 8px 9px 12px; cursor: pointer; transition: background .12s; }
.pl-item + .pl-item { border-top: 1px solid var(--color-border); }
.pl-item:hover { background: color-mix(in srgb, var(--color-hover) 55%, transparent); }
.pl-item.pl-on { background: var(--color-accent-soft); }
.pl-item-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.pl-item-title { font-size: 12px; font-weight: 600; color: var(--color-fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pl-item-text { font-size: 11px; color: var(--color-fg-subtle); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pl-item-meta { display: flex; gap: 4px; flex-wrap: wrap; align-items: center; font-size: 10px; color: var(--color-fg-subtle); }
.pl-tag { display: inline-flex; align-items: center; height: 16px; padding: 0 5px; border-radius: 4px; font-size: 10px; font-weight: 500; color: var(--color-fg-subtle); background: var(--color-active); }
.pl-icon-btn { display: inline-flex; align-items: center; justify-content: center; width: 26px; height: 26px; flex-shrink: 0; border-radius: 6px; border: 0; background: transparent; color: var(--color-fg-subtle); }
.pl-icon-btn:hover { background: var(--color-active); color: var(--color-accent); }
.pl-icon-btn .pl-icon { width: 13px; height: 13px; }
.pl-item .pl-icon-btn { opacity: 0; }
.pl-item:hover .pl-icon-btn, .pl-item.pl-on .pl-icon-btn { opacity: 1; }
.pl-editor { display: flex; flex-direction: column; }
.pl-title-input { border: 0; outline: none; background: transparent; padding: 14px 16px 4px; font: inherit; font-size: 16px; font-weight: 600; color: var(--color-fg); user-select: text; }
.pl-tags-input { border: 0; outline: none; background: transparent; padding: 0 16px 10px; font: inherit; font-size: 12px; color: var(--color-fg-muted); user-select: text; }
.pl-title-input::placeholder, .pl-tags-input::placeholder, .pl-body::placeholder { color: var(--color-fg-subtle); }
.pl-body { min-height: 220px; resize: vertical; padding: 12px 16px; border: 0; border-top: 1px solid var(--color-border); outline: none; background: transparent; color: var(--color-fg); font-family: var(--font-mono); font-size: 12.5px; line-height: 1.65; user-select: text; }
.pl-vars { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; padding: 8px 16px; border-top: 1px solid var(--color-border); font-size: 11px; color: var(--color-fg-subtle); }
.pl-var { display: inline-flex; align-items: center; gap: 4px; height: 20px; padding: 0 7px; border-radius: 5px; font-family: var(--font-mono); font-size: 11px; color: var(--color-accent); background: var(--color-accent-soft); }
.pl-var.pl-auto { color: #2bb39e; background: color-mix(in srgb, #2bb39e 13%, transparent); }
.pl-var small { color: var(--color-fg-subtle); font-family: var(--font-sans); }
.pl-bar { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; padding: 8px 10px; border-top: 1px solid var(--color-border); background: var(--color-panel); }
.pl-grow { flex: 1; }
.pl-saved { font-size: 11px; color: var(--color-fg-subtle); }
.pl-select { height: 28px; max-width: 240px; padding: 0 6px; border-radius: 6px; border: 1px solid var(--color-border); background: var(--color-panel); color: var(--color-fg-muted); font: inherit; font-size: 12px; }
.pl-kbd { font-family: var(--font-mono); font-size: 10px; padding: 1px 5px; border-radius: 4px; background: rgb(255 255 255 / .18); }
.pl-hint { font-size: 11px; line-height: 1.6; color: var(--color-fg-subtle); padding: 0 2px; }
.pl-hint code { font-family: var(--font-mono); font-size: 11px; padding: 0 4px; border-radius: 4px; background: var(--color-active); color: var(--color-fg); }
.pl-empty { display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 36px 20px; text-align: center; font-size: 12px; color: var(--color-fg-subtle); }
.pl-empty b { font-size: 13px; color: var(--color-fg); }

.pl-dialog { width: min(560px, 92vw); padding: 0; gap: 0; }
.pl-dialog-head { display: flex; align-items: center; gap: 12px; padding: 18px 20px 14px; }
.pl-dialog-title { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.pl-dialog-body { display: flex; flex-direction: column; gap: 12px; padding: 4px 20px 18px; max-height: 60vh; overflow: auto; }
.pl-dialog-foot { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; padding: 12px 20px; border-top: 1px solid var(--color-border); background: var(--color-panel); }
.pl-field { display: flex; flex-direction: column; gap: 6px; font-size: 12px; font-weight: 500; color: var(--color-fg-muted); }
.pl-mono { font-family: var(--font-mono); }
.pl-text { min-height: 32px; padding: 7px 10px; border-radius: 8px; border: 1px solid var(--color-border-strong); background: var(--color-panel); color: var(--color-fg); font: inherit; font-size: 12px; outline: none; resize: vertical; user-select: text; }
.pl-text:focus { border-color: var(--color-accent); box-shadow: 0 0 0 3px var(--color-accent-soft); }
.pl-preview { padding: 10px 12px; border-radius: 8px; border: 1px dashed var(--color-border-strong); font-family: var(--font-mono); font-size: 11.5px; line-height: 1.6; font-weight: 400; color: var(--color-fg); white-space: pre-wrap; max-height: 200px; overflow: auto; user-select: text; }
.pl-submit { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--color-fg-muted); }
.pl-import { width: 100%; min-height: 220px; font-family: var(--font-mono); }
.pl-alert { display: flex; gap: 8px; align-items: flex-start; padding: 9px 12px; border-radius: 8px; font-size: 12px; color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 9%, transparent); border: 1px solid color-mix(in srgb, var(--color-danger) 25%, transparent); }
`;

function mountPage(root, api, store, actions) {
  let selected = actions.select ?? store.all()[0]?.id ?? null;
  let query = "";
  let tag = "";
  let savedAt = 0;
  let saveTimer = null;

  const page = h("div", { class: "wf-page pl-page" });
  root.append(page);

  const matches = (p) => {
    if (tag && !p.tags.includes(tag)) return false;
    if (!query) return true;
    return [p.title, p.body, p.tags.join(" ")].join(" ").toLowerCase().includes(query.toLowerCase());
  };

  const search = h("input", {
    type: "search",
    placeholder: "Search prompts",
    spellcheck: "false",
    oninput: (e) => {
      query = e.target.value;
      renderList();
    },
  });
  const chips = h("div", { class: "pl-chips" });
  const list = h("div", { class: "wf-card pl-list" });
  const main = h("div", { class: "pl-main" });

  // The editor's fields stay put while typing; only their values change on selection.
  const titleInput = h("input", { class: "pl-title-input", placeholder: "Title", spellcheck: "false", oninput: () => edit({ title: titleInput.value }) });
  const tagsInput = h("input", { class: "pl-tags-input", placeholder: "Tags, separated by commas", spellcheck: "false", oninput: () => edit({ tags: parseTags(tagsInput.value) }) });
  const body = h("textarea", {
    class: "pl-body",
    spellcheck: "false",
    placeholder: "Write the prompt. Use {{name}} for what changes each time, e.g. Write tests for {{file}}.",
    oninput: () => {
      edit({ body: body.value });
      renderVars();
    },
    onkeydown: (e) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        sendCurrent();
      }
    },
  });
  const vars = h("div", { class: "pl-vars" });
  const saved = h("span", { class: "pl-saved" });
  let target = actions.targetPicker();

  const editor = h(
    "div",
    { class: "wf-card pl-editor" },
    titleInput,
    tagsInput,
    body,
    vars,
    h(
      "div",
      { class: "pl-bar" },
      h("button", { class: "wf-button wf-primary", onclick: () => sendCurrent() }, icon("send"), "Send", h("span", { class: "pl-kbd" }, "⌘↵")),
      target,
      h("span", { class: "pl-grow" }),
      saved,
      h("button", { class: "wf-button wf-ghost", title: "Duplicate", onclick: () => duplicate() }, icon("copy"), "Duplicate"),
      h("button", { class: "wf-button wf-ghost", title: "Delete", onclick: () => removeCurrent() }, icon("trash")),
    ),
  );
  const hint = h(
    "div",
    { class: "pl-hint" },
    "Placeholders: ",
    h("code", {}, "{{name}}"),
    " asks for a value, ",
    h("code", {}, "{{name:default}}"),
    " suggests one, and ",
    Object.keys(BUILTINS).flatMap((b, i) => [i ? " " : "", h("code", { title: BUILTINS[b] }, `{{${b}}}`)]),
    " fill themselves. Every prompt is in ⌘K as “Prompt: …”, and you can ask for one by voice.",
  );

  page.append(
    h(
      "div",
      { class: "pl-hero" },
      h("div", { class: "pl-logo" }, icon("text")),
      h("div", { class: "pl-hero-text" }, h("h1", { class: "wf-h1" }, "Prompts"), h("div", { class: "wf-muted" }, "The prompts you keep retyping, a keystroke away for any agent.")),
      h("button", { class: "wf-button wf-ghost", onclick: () => openImport() }, icon("upload"), "Import"),
      h("button", { class: "wf-button wf-ghost", onclick: () => exportAll() }, icon("download"), "Export"),
      h("button", { class: "wf-button wf-primary", onclick: () => create() }, icon("plus"), "New prompt"),
    ),
    h("div", { class: "pl-grid" }, h("div", { class: "pl-side" }, h("label", { class: "pl-search" }, icon("search"), search), chips, list), main),
  );

  const current = () => store.get(selected);

  function edit(fields) {
    if (!selected) return;
    store.update(selected, fields);
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      savedAt = Date.now();
      saved.textContent = "Saved";
    }, 400);
  }

  function create() {
    const p = store.add({ tags: tag ? [tag] : [] });
    selected = p.id;
    query = "";
    search.value = "";
    render();
    titleInput.select();
    titleInput.focus();
  }

  function duplicate() {
    const p = current();
    if (!p) return;
    selected = store.add({ title: `${p.title} (copy)`, body: p.body, tags: p.tags }).id;
    render();
  }

  async function removeCurrent() {
    const p = current();
    if (!p) return;
    if (!(await api.confirm(`Delete “${p.title}”?`, { title: "Delete prompt", okLabel: "Delete" }))) return;
    store.remove(p.id);
    selected = store.all().find(matches)?.id ?? store.all()[0]?.id ?? null;
    render();
  }

  function targetValue() {
    return target.value.startsWith("pane:") ? { paneId: target.value.slice(5) } : target.value;
  }

  function sendCurrent() {
    const p = current();
    if (!p) return;
    if (parseVars(p.body).length) actions.openFill(p, targetValue());
    else void actions.send(p, {}, targetValue());
  }

  async function exportAll() {
    const data = JSON.stringify({ prompts: store.all().map(({ id, title, body, tags }) => ({ id, title, body, tags })) }, null, 2);
    try {
      await navigator.clipboard.writeText(data);
      api.toast(`Copied ${store.all().length} prompts as JSON`, "success");
    } catch (err) {
      api.toast(String(err?.message ?? err), "error");
    }
  }

  function openImport() {
    const area = h("textarea", { class: "pl-text pl-import", spellcheck: "false", placeholder: '{ "prompts": [ { "title": "…", "body": "…", "tags": [] } ] }' });
    const problem = h("div", { class: "pl-alert", style: "display:none" });
    const close = () => backdrop.remove();
    const go = (e) => {
      e.preventDefault();
      try {
        const incoming = normalizePrompts(JSON.parse(area.value));
        store.merge(incoming);
        api.toast(`Imported ${incoming.length} prompt${incoming.length === 1 ? "" : "s"}`, "success");
        selected = incoming[0]?.id ?? selected;
        close();
        render();
      } catch (err) {
        problem.replaceChildren(icon("alert"), String(err?.message ?? err));
        problem.style.display = "";
      }
    };
    const backdrop = h(
      "div",
      { class: "wf-dialog-backdrop", onmousedown: (e) => e.target === backdrop && close() },
      h(
        "form",
        { class: "wf-dialog pl-dialog", onsubmit: go, onkeydown: (e) => e.key === "Escape" && (e.stopPropagation(), close()) },
        h("div", { class: "pl-dialog-head" }, h("div", { class: "pl-logo" }, icon("upload")), h("div", { class: "pl-dialog-title" }, h("h2", { class: "wf-h2", style: "font-weight:600" }, "Import prompts"), h("div", { class: "wf-subtle" }, "Paste JSON exported from a Prompt Library. Prompts with the same id are replaced."))),
        h("div", { class: "pl-dialog-body" }, area, problem),
        h("div", { class: "pl-dialog-foot" }, h("span", { class: "pl-grow" }), h("button", { class: "wf-button wf-ghost", type: "button", onclick: close }, "Cancel"), h("button", { class: "wf-button wf-primary", type: "submit" }, "Import")),
      ),
    );
    document.body.append(backdrop);
    area.focus();
  }

  function renderList() {
    const all = store.all();
    const tags = [...new Set(all.flatMap((p) => p.tags))].sort();
    if (tag && !tags.includes(tag)) tag = "";
    chips.replaceChildren(...(tags.length ? [h("button", { class: `pl-chip${tag ? "" : " pl-on"}`, onclick: () => ((tag = ""), renderList()) }, "All")] : []), ...tags.map((t) => h("button", { class: `pl-chip${t === tag ? " pl-on" : ""}`, onclick: () => ((tag = t === tag ? "" : t), renderList()) }, t)));
    const shown = all.filter(matches);
    list.replaceChildren(
      ...(shown.length === 0
        ? [h("div", { class: "pl-empty" }, all.length ? `Nothing matches${query ? ` “${query}”` : ""}.` : "No prompts yet.")]
        : shown.map((p) =>
            h(
              "div",
              {
                class: `pl-item${p.id === selected ? " pl-on" : ""}`,
                onclick: () => {
                  selected = p.id;
                  render();
                },
              },
              h(
                "div",
                { class: "pl-item-body" },
                h("div", { class: "pl-item-title" }, p.title || "Untitled prompt"),
                h("div", { class: "pl-item-text" }, p.body.replace(/\s+/g, " ") || "Empty"),
                h("div", { class: "pl-item-meta" }, p.tags.map((t) => h("span", { class: "pl-tag" }, t)), p.uses ? `${p.uses}× · ${ago(p.lastUsed)}` : "never used"),
              ),
              h("button", { class: "pl-icon-btn", title: "Send", "aria-label": `Send ${p.title}`, onclick: (e) => (e.stopPropagation(), actions.use(p, targetValue())) }, icon("send")),
            ),
          )),
    );
  }

  function renderVars() {
    const p = current();
    const own = parseVars(p?.body ?? "");
    const auto = usedBuiltins(p?.body ?? "");
    vars.replaceChildren(
      ...(own.length || auto.length
        ? [
            h("span", {}, "Fills in:"),
            ...own.map((v) => h("span", { class: "pl-var", title: v.def ? `Default: ${v.def}` : "You fill this in when sending" }, icon("braces"), v.name, v.def ? h("small", {}, `= ${v.def}`) : null)),
            ...auto.map((b) => h("span", { class: "pl-var pl-auto", title: BUILTINS[b] }, icon("bolt"), b)),
          ]
        : [h("span", {}, "No placeholders: it's sent as is.")]),
    );
  }

  function render() {
    renderList();
    const p = current();
    if (!p) {
      main.replaceChildren(h("div", { class: "wf-card" }, h("div", { class: "pl-empty" }, h("b", {}, "No prompt selected"), "Write one with New prompt.")), hint);
      return;
    }
    if (main.firstChild !== editor) main.replaceChildren(editor, hint);
    if (titleInput.value !== p.title) titleInput.value = p.title;
    if (document.activeElement !== tagsInput) tagsInput.value = p.tags.join(", ");
    if (body.value !== p.body) body.value = p.body;
    saved.textContent = savedAt ? "Saved" : `Edited ${ago(p.updatedAt)}`;
    // Agents come and go; refresh the target list each time.
    const fresh = actions.targetPicker(targetValue());
    target.replaceWith(fresh);
    target = fresh;
    renderVars();
  }

  // Changes from elsewhere (an agent saving a prompt, uses counted) redraw the list.
  const unsubscribe = store.subscribe(() => renderList());
  render();
  return () => {
    unsubscribe();
    clearTimeout(saveTimer);
    page.remove();
  };
}
