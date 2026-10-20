// Secrets: a page for keeping API keys, environment variables and logins in the OS keychain,
// and agent tools that use them by name. Values stay in winfer's Rust side: agents get command
// output with the values redacted, or a browser field filled in, never the value itself.

const KINDS = {
  env: "Environment variable",
  token: "API key / token",
  login: "Login",
  other: "Other",
};

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

const keyName = (text) => text.toUpperCase().replace(/[^A-Z0-9_]+/g, "_").replace(/^(?=\d)/, "_");
const needsApproval = (secret) => secret.meta.approve !== false;

function describe(secret) {
  const parts = [`${secret.key} (${KINDS[secret.meta.kind] ?? "secret"})`];
  if (secret.meta.username) parts.push(`username ${secret.meta.username}`);
  if (secret.meta.description) parts.push(String(secret.meta.description));
  if (needsApproval(secret)) parts.push("the user approves each use");
  return parts.join(" — ");
}

export function activate(api) {
  /** Secrets by key, for the agent tools. */
  async function find(keys) {
    const all = await api.vault.list();
    const missing = keys.filter((k) => !all.some((s) => s.key === k));
    if (missing.length) {
      const known = all.map((s) => s.key).join(", ") || "none yet";
      throw new Error(`Unknown secret ${missing.join(", ")}. Stored secrets: ${known}. The user can add secrets on winfer's Secrets page.`);
    }
    return all.filter((s) => keys.includes(s.key));
  }

  /** Asks the user when any of `secrets` wants approval; throws if they decline. */
  async function approve(secrets, caller, action) {
    const asking = secrets.filter(needsApproval);
    if (asking.length === 0) return;
    const ok = await api.confirm(`${caller.label} in “${caller.workspaceName}” wants to ${action}\n\nusing ${asking.map((s) => s.key).join(", ")}.`, {
      title: "Allow use of secrets?",
      okLabel: "Allow",
      caller,
    });
    if (!ok) throw new Error("The user declined using these secrets.");
  }

  api.registerAgentTool({
    name: "list_secrets",
    description:
      "List the secrets (API keys, tokens, environment variables, logins) the user stored in winfer, by name. Values are never shown; use run_with_secrets or fill_secret_in_browser to use them.",
    inputSchema: { type: "object", properties: {} },
    async run() {
      const all = await api.vault.list();
      if (all.length === 0) return "No secrets stored yet. Ask the user to add them on winfer's Secrets page.";
      return all.map(describe).join("\n");
    },
  });

  api.registerAgentTool({
    name: "run_with_secrets",
    description:
      "Run a shell command with stored secrets as environment variables (reference them as $NAME; %NAME% on Windows), e.g. `curl -H \"Authorization: Bearer $API_TOKEN\" …` or `npm publish`. Runs in your working directory without a terminal. The output comes back with the secret values replaced by [secret NAME]. Never try to print or transform a secret's value.",
    inputSchema: {
      type: "object",
      properties: {
        command: { type: "string", description: "Shell command to run" },
        secrets: { type: "array", items: { type: "string" }, description: "Names of the secrets to provide as environment variables" },
        cwd: { type: "string", description: "Absolute directory to run in (default: your working directory)" },
        timeout_sec: { type: "integer", description: "Stop after this many seconds (default 120, max 1800)" },
      },
      required: ["command", "secrets"],
    },
    async run(args, caller) {
      const command = String(args.command ?? "").trim();
      if (!command) throw new Error("command is empty");
      const keys = Array.isArray(args.secrets) ? args.secrets.map(String) : [];
      const secrets = await find(keys);
      await approve(secrets, caller, `run:\n\n${command}`);
      const cwd = typeof args.cwd === "string" && /^([a-zA-Z]:)?[\\/]/.test(args.cwd) ? args.cwd : caller.cwd;
      const result = await api.vault.exec({ command, cwd, keys, timeoutSec: Number(args.timeout_sec) || undefined });
      const status = result.timedOut ? "Stopped at the timeout." : `Exit code ${result.code ?? "unknown"}.`;
      return `${status}\n\n${result.output || "(no output)"}`;
    },
  });

  api.registerAgentTool({
    name: "fill_secret_in_browser",
    description:
      "Type a stored secret (e.g. a password or API key) into a text field of the page open in winfer's browser panel, found by CSS selector, without you seeing it. Use list_secrets for names; a login's username is listed there.",
    inputSchema: {
      type: "object",
      properties: {
        secret: { type: "string", description: "Name of the secret" },
        selector: { type: "string", description: "CSS selector of the field, e.g. input[type=password]" },
        tab: { type: "string", description: "Part of the tab's URL, if not the active tab" },
      },
      required: ["secret", "selector"],
    },
    async run(args, caller) {
      const [secret] = await find([String(args.secret ?? "")]);
      const selector = String(args.selector ?? "");
      if (!selector) throw new Error("selector is empty");
      await approve([secret], caller, `type ${secret.key} into ${selector} in the browser`);
      const url = await api.vault.fill({ workspaceId: caller.workspaceId, key: secret.key, selector, tab: args.tab ? String(args.tab) : undefined });
      return `Typed ${secret.key} into ${selector} on ${url}.`;
    },
  });

  api.registerPage({ id: "secrets", title: "Secrets", icon: "key-round", mount: (el) => mountPage(el, api) });

  api.registerTool({
    id: "open",
    title: "Secrets",
    description: "Open the secrets page to add, change or remove stored API keys, passwords and environment variables",
    palette: [{ title: "Secrets: manage stored secrets" }],
    run: () => api.openPage("secrets"),
  });
}

const CSS = `
.sc-page { max-width: 820px; gap: 16px; container-type: inline-size; }
.sc-icon { width: 14px; height: 14px; flex-shrink: 0; }
.sc-hero { display: flex; align-items: center; gap: 14px; }
.sc-hero-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.sc-logo { width: 40px; height: 40px; border-radius: 11px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; color: white; background: linear-gradient(160deg, #f5c451, #d9902a); box-shadow: inset 0 1px 0 rgb(255 255 255 / .25), 0 4px 14px rgb(217 144 42 / .25); }
.sc-logo .sc-icon { width: 20px; height: 20px; }
.sc-pill { display: inline-flex; align-items: center; gap: 6px; height: 24px; padding: 0 10px; border-radius: 999px; font-size: 11px; font-weight: 500; white-space: nowrap; color: var(--color-fg-muted); background: var(--color-panel-raised); border: 1px solid var(--color-border); }
@container (max-width: 600px) { .sc-pill { display: none; } }
.sc-pill .sc-icon { width: 12px; height: 12px; color: var(--color-success); }
.sc-alert { display: flex; gap: 8px; align-items: flex-start; padding: 9px 12px; border-radius: 8px; font-size: 12px; line-height: 1.5; color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 9%, transparent); border: 1px solid color-mix(in srgb, var(--color-danger) 25%, transparent); }
.sc-alert .sc-icon { margin-top: 2px; }
.sc-section { display: flex; flex-direction: column; gap: 8px; }
.sc-label { display: flex; align-items: center; gap: 8px; font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: .05em; color: var(--color-fg-subtle); padding-left: 2px; }
.sc-label span { font-weight: 500; letter-spacing: 0; text-transform: none; }

.sc-toolbar { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.sc-search { flex: 1; min-width: 180px; display: flex; align-items: center; gap: 8px; height: 30px; padding: 0 10px; border-radius: 8px; border: 1px solid var(--color-border-strong); background: var(--color-panel); transition: border-color .15s, box-shadow .15s; }
.sc-search:focus-within { border-color: var(--color-accent); box-shadow: 0 0 0 3px var(--color-accent-soft); }
.sc-search > .sc-icon { color: var(--color-fg-subtle); }
.sc-search input { flex: 1; min-width: 0; height: 100%; border: 0; outline: none; background: transparent; color: var(--color-fg); font: inherit; font-size: 12px; user-select: text; }
.sc-search input::placeholder { color: var(--color-fg-subtle); }
.sc-search input::-webkit-search-cancel-button { display: none; }
.sc-chips { display: flex; gap: 4px; flex-wrap: wrap; }
.sc-chip { display: inline-flex; align-items: center; gap: 6px; height: 26px; padding: 0 10px; border-radius: 999px; border: 1px solid var(--color-border); background: transparent; font: inherit; font-size: 11px; font-weight: 500; color: var(--color-fg-muted); }
.sc-chip:hover { border-color: var(--color-border-strong); color: var(--color-fg); }
.sc-chip b { font-weight: 500; font-variant-numeric: tabular-nums; color: var(--color-fg-subtle); }
.sc-chip.sc-on { border-color: color-mix(in srgb, var(--color-accent) 50%, transparent); background: var(--color-accent-soft); color: var(--color-fg); }
.sc-chip.sc-on b { color: var(--color-accent); }

.sc-kind { --sc-tone: var(--color-fg-subtle); width: 32px; height: 32px; border-radius: 8px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; color: var(--sc-tone); background: color-mix(in srgb, var(--sc-tone) 13%, transparent); border: 1px solid color-mix(in srgb, var(--sc-tone) 22%, transparent); }
.sc-kind.sc-env { --sc-tone: #2bb39e; }
.sc-kind.sc-token { --sc-tone: var(--color-accent); }
.sc-kind.sc-login { --sc-tone: #e08561; }
.sc-kind.sc-other { --sc-tone: #b48ae8; }

.sc-row { display: flex; align-items: flex-start; gap: 12px; padding: 12px 10px 12px 14px; transition: background .12s; }
.sc-row + .sc-row { border-top: 1px solid var(--color-border); }
.sc-row:hover { background: color-mix(in srgb, var(--color-hover) 55%, transparent); }
.sc-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; padding-top: 1px; }
.sc-title { display: flex; align-items: center; gap: 8px; min-width: 0; }
.sc-name { font-family: var(--font-mono); font-size: 13px; font-weight: 500; color: var(--color-fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; user-select: text; }
.sc-tag { display: inline-flex; align-items: center; gap: 4px; height: 18px; padding: 0 6px; border-radius: 5px; font-size: 10px; font-weight: 500; white-space: nowrap; color: var(--color-fg-subtle); background: var(--color-active); }
.sc-tag .sc-icon { width: 11px; height: 11px; }
.sc-tag.sc-ask { color: var(--color-accent); background: var(--color-accent-soft); }
.sc-tag.sc-auto { color: var(--color-warning); background: color-mix(in srgb, var(--color-warning) 12%, transparent); }
.sc-meta { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; font-size: 12px; color: var(--color-fg-muted); line-height: 1.5; }
.sc-meta .sc-icon { width: 12px; height: 12px; color: var(--color-fg-subtle); }
.sc-sep { color: var(--color-fg-subtle); }
.sc-value { display: flex; align-items: center; gap: 8px; margin-top: 4px; padding: 6px 6px 6px 10px; border-radius: 7px; background: var(--color-panel); border: 1px solid var(--color-border); }
.sc-value code { flex: 1; min-width: 0; font-family: var(--font-mono); font-size: 12px; color: var(--color-fg); word-break: break-all; user-select: text; }
.sc-dots { font-family: var(--font-mono); font-size: 12px; letter-spacing: .1em; color: var(--color-fg-subtle); white-space: nowrap; padding-top: 7px; }
.sc-actions { display: flex; gap: 2px; flex-shrink: 0; }
.sc-icon-btn { display: inline-flex; align-items: center; justify-content: center; width: 28px; height: 28px; flex-shrink: 0; border-radius: 6px; border: 0; background: transparent; color: var(--color-fg-subtle); }
.sc-icon-btn:hover { background: var(--color-active); color: var(--color-fg); }
.sc-icon-btn.sc-bad:hover { color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 12%, transparent); }
.sc-icon-btn.sc-done { color: var(--color-success); }
@container (max-width: 520px) { .sc-dots { display: none; } }

.sc-empty { display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 36px 20px 28px; text-align: center; }
.sc-empty .sc-logo { margin-bottom: 8px; }
.sc-empty-title { font-size: 13px; font-weight: 600; color: var(--color-fg); }
.sc-starters { display: flex; gap: 8px; flex-wrap: wrap; justify-content: center; margin-top: 14px; }
.sc-starter { display: flex; align-items: center; gap: 10px; padding: 8px 12px 8px 8px; border-radius: 9px; border: 1px solid var(--color-border); background: var(--color-panel); font: inherit; font-size: 12px; font-weight: 500; color: var(--color-fg); }
.sc-starter:hover { border-color: var(--color-border-strong); background: var(--color-hover); }
.sc-starter .sc-kind { width: 26px; height: 26px; border-radius: 7px; }
.sc-nomatch { padding: 28px 16px; text-align: center; font-size: 12px; color: var(--color-fg-subtle); }

.sc-tools { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); }
@container (max-width: 620px) { .sc-tools { grid-template-columns: minmax(0, 1fr); } }
.sc-tool { display: flex; flex-direction: column; gap: 8px; padding: 14px 16px; }
.sc-tool + .sc-tool { border-left: 1px solid var(--color-border); }
@container (max-width: 620px) { .sc-tool + .sc-tool { border-left: 0; border-top: 1px solid var(--color-border); } }
.sc-tool-head { display: flex; align-items: center; gap: 8px; }
.sc-tool-head .sc-icon { color: var(--color-accent); }
.sc-code { font-family: var(--font-mono); font-size: 11px; padding: 1px 6px; border-radius: 4px; background: var(--color-active); color: var(--color-fg); user-select: text; }
.sc-tool p { margin: 0; font-size: 12px; line-height: 1.55; color: var(--color-fg-muted); }
.sc-note { display: flex; gap: 8px; align-items: flex-start; padding: 10px 16px; border-top: 1px solid var(--color-border); font-size: 11px; line-height: 1.55; color: var(--color-fg-subtle); background: color-mix(in srgb, var(--color-warning) 4%, transparent); }
.sc-note .sc-icon { width: 12px; height: 12px; margin-top: 2px; color: var(--color-warning); }

.sc-dialog { width: min(500px, 92vw); padding: 0; gap: 0; }
.sc-dialog-head { display: flex; align-items: center; gap: 12px; padding: 18px 20px 14px; }
.sc-dialog-title { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.sc-dialog-body { display: flex; flex-direction: column; gap: 14px; padding: 4px 20px 18px; }
.sc-dialog-foot { display: flex; align-items: center; justify-content: flex-end; gap: 8px; padding: 12px 20px; border-top: 1px solid var(--color-border); background: var(--color-panel); }
.sc-field { display: flex; flex-direction: column; gap: 6px; font-size: 12px; font-weight: 500; color: var(--color-fg-muted); }
.sc-field small { font-size: 11px; font-weight: 400; color: var(--color-fg-subtle); }
.sc-input { display: flex; align-items: center; gap: 6px; height: 32px; padding: 0 4px 0 10px; border-radius: 8px; border: 1px solid var(--color-border-strong); background: var(--color-panel); transition: border-color .15s, box-shadow .15s; }
.sc-input:focus-within { border-color: var(--color-accent); box-shadow: 0 0 0 3px var(--color-accent-soft); }
.sc-input.sc-locked { opacity: .7; }
.sc-input > span { font-family: var(--font-mono); font-size: 12px; color: var(--color-fg-subtle); }
.sc-input input { flex: 1; min-width: 0; height: 100%; border: 0; outline: none; background: transparent; color: var(--color-fg); font: inherit; font-size: 12px; user-select: text; }
.sc-input input.sc-mono { font-family: var(--font-mono); }
.sc-input input::placeholder { color: var(--color-fg-subtle); font-family: var(--font-sans); }
.sc-input .sc-icon-btn { width: 24px; height: 24px; }
.sc-kinds { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 6px; }
.sc-kind-opt { display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 10px 6px 8px; border-radius: 9px; border: 1px solid var(--color-border); background: var(--color-panel); font: inherit; font-size: 11px; font-weight: 500; color: var(--color-fg-muted); text-align: center; transition: border-color .15s, background .15s; }
.sc-kind-opt:hover { border-color: var(--color-border-strong); color: var(--color-fg); }
.sc-kind-opt .sc-kind { width: 26px; height: 26px; border-radius: 7px; }
.sc-kind-opt.sc-on { border-color: var(--color-accent); background: color-mix(in srgb, var(--color-accent) 8%, var(--color-panel)); color: var(--color-fg); }
.sc-kind-opt:focus-visible { outline: none; box-shadow: 0 0 0 3px var(--color-accent-soft); }
.sc-approve { display: flex; align-items: center; gap: 12px; padding: 10px 12px; border-radius: 9px; border: 1px solid var(--color-border); background: var(--color-panel); }
.sc-approve > .sc-icon { width: 16px; height: 16px; color: var(--color-accent); }
.sc-approve.sc-off > .sc-icon { color: var(--color-warning); }
.sc-approve > div { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.sc-approve b { font-size: 12px; font-weight: 500; color: var(--color-fg); }
.sc-approve span { font-size: 11px; line-height: 1.45; color: var(--color-fg-subtle); }
`;

const ICONS = {
  key: "M15.5 7.5 19 4m2-2-2 2m0 0 3 3-3.5 3.5-3-3M11.4 11.6a5.5 5.5 0 1 1-7.8 7.8 5.5 5.5 0 0 1 7.8-7.8z",
  terminal: "M4 17l6-6-6-6M12 19h8",
  user: "M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2M12 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
  lock: "M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4",
  eye: "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
  eyeOff: "M9.9 9.9a3 3 0 1 0 4.2 4.2M10.7 5.1A10 10 0 0 1 12 5c7 0 10 7 10 7a13 13 0 0 1-1.7 2.7M6.6 6.6A13.5 13.5 0 0 0 2 12s3 7 10 7a9.7 9.7 0 0 0 5.4-1.6M2 2l20 20",
  copy: "M9 9h11v11H9zM5 15H4V4h11v1",
  check: "M20 6 9 17l-5-5",
  pencil: "M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z",
  trash: "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6",
  search: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3",
  plus: "M12 5v14M5 12h14",
  shield: "M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10zM9 12l2 2 4-4",
  bolt: "M13 2 3 14h9l-1 8 10-12h-9z",
  list: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
  browser: "M3 4h18v16H3zM3 9h18M7 6.5h.01M10 6.5h.01",
  alert: "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
};

function icon(name) {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("class", "sc-icon");
  for (const [k, v] of Object.entries({ fill: "none", stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round", "stroke-linejoin": "round" })) el.setAttribute(k, v);
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", ICONS[name]);
  el.append(path);
  return el;
}

/** How each kind looks: icon, short label, and the hints in the editor. */
const KIND_UI = {
  env: { icon: "terminal", short: "Env var", value: "Value", placeholder: "DATABASE_URL" },
  token: { icon: "key", short: "API key", value: "Key or token", placeholder: "OPENAI_API_KEY" },
  login: { icon: "user", short: "Login", value: "Password", placeholder: "GITHUB_LOGIN" },
  other: { icon: "lock", short: "Other", value: "Value", placeholder: "SIGNING_KEY" },
};
const kindOf = (secret) => (KIND_UI[secret.meta.kind] ? secret.meta.kind : "other");
const kindTile = (kind) => h("div", { class: `sc-kind sc-${kind}` }, icon(KIND_UI[kind].icon));

const TOOLS = [
  { icon: "list", name: "list_secrets", text: "Sees the names, kinds, usernames and descriptions. Never the values." },
  { icon: "terminal", name: "run_with_secrets", text: "Runs a command with secrets as $NAME. Their values are cut from the output." },
  { icon: "browser", name: "fill_secret_in_browser", text: "Has winfer type a secret into a field in the browser panel." },
];

function mountPage(root, api) {
  let secrets = [];
  let query = "";
  let filter = "all";
  let error = "";
  let copied = null;
  /** Revealed values by key, cleared when the page closes. */
  const revealed = new Map();
  const style = h("style", {}, CSS);
  const page = h("div", { class: "wf-page sc-page" });
  root.append(style, page);

  async function refresh() {
    try {
      secrets = await api.vault.list();
      error = "";
    } catch (err) {
      error = String(err);
    }
    render();
  }

  async function act(work) {
    try {
      await work();
    } catch (err) {
      api.toast(String(err?.message ?? err), "error");
    }
    await refresh();
  }

  async function copy(secret) {
    try {
      await navigator.clipboard.writeText(await api.vault.reveal(secret.key));
      copied = secret.key;
      render();
      setTimeout(() => copied === secret.key && ((copied = null), render()), 1500);
    } catch (err) {
      api.toast(String(err?.message ?? err), "error");
    }
  }

  const button = (name, title, onclick, extra = "") => h("button", { class: `sc-icon-btn ${extra}`, title, "aria-label": title, onclick }, icon(name));

  function row(secret) {
    const value = revealed.get(secret.key);
    const meta = secret.meta;
    const kind = kindOf(secret);
    const details = [
      meta.username && h("span", { class: "wf-inline", style: "gap:4px" }, icon("user"), meta.username),
      meta.description && h("span", {}, meta.description),
    ].filter(Boolean);
    return h(
      "div",
      { class: "sc-row" },
      kindTile(kind),
      h(
        "div",
        { class: "sc-body" },
        h(
          "div",
          { class: "sc-title" },
          h("span", { class: "sc-name", title: secret.key }, secret.key),
          h("span", { class: "sc-tag" }, KIND_UI[kind].short),
          needsApproval(secret)
            ? h("span", { class: "sc-tag sc-ask", title: "Agents need your OK each time they use it" }, icon("shield"), "Asks first")
            : h("span", { class: "sc-tag sc-auto", title: "Agents can use it without asking you" }, icon("bolt"), "Auto"),
        ),
        details.length > 0 && h("div", { class: "sc-meta" }, details.flatMap((d, i) => (i ? [h("span", { class: "sc-sep" }, "·"), d] : [d]))),
        value !== undefined && h("div", { class: "sc-value" }, h("code", {}, value || "(empty)")),
      ),
      value === undefined && h("span", { class: "sc-dots", "aria-hidden": "true" }, "••••••••••"),
      h(
        "div",
        { class: "sc-actions" },
        button(value === undefined ? "eye" : "eyeOff", value === undefined ? "Show value" : "Hide value", () =>
          act(async () => {
            if (revealed.has(secret.key)) revealed.delete(secret.key);
            else revealed.set(secret.key, await api.vault.reveal(secret.key));
          }),
        ),
        copied === secret.key ? button("check", "Copied", () => {}, "sc-done") : button("copy", "Copy value", () => copy(secret)),
        button("pencil", "Edit", () => openEditor(secret)),
        button(
          "trash",
          "Delete",
          () =>
            act(async () => {
              if (await api.confirm(`Delete ${secret.key}? Its value is removed from your keychain.`, { title: "Delete secret", okLabel: "Delete" })) {
                revealed.delete(secret.key);
                await api.vault.delete(secret.key);
              }
            }),
          "sc-bad",
        ),
      ),
    );
  }

  function empty() {
    return h(
      "div",
      { class: "sc-empty" },
      h("div", { class: "sc-logo" }, icon("key")),
      h("div", { class: "sc-empty-title" }, "No secrets yet"),
      h("div", { class: "wf-muted", style: "max-width:380px" }, "Add the keys and logins your agents need. They stay in your keychain; agents only ever get the names."),
      h(
        "div",
        { class: "sc-starters" },
        ["env", "token", "login"].map((kind) => h("button", { class: "sc-starter", onclick: () => openEditor(null, kind) }, kindTile(kind), KINDS[kind])),
      ),
    );
  }

  const pill = h("span", { class: "sc-pill" });
  const problem = h("div", { class: "sc-alert" });
  const search = h("input", {
    type: "search",
    placeholder: "Search by name, username or description",
    spellcheck: "false",
    oninput: (e) => {
      query = e.target.value;
      render();
    },
    onkeydown: (e) => e.key === "Escape" && query && (e.stopPropagation(), (e.target.value = query = ""), render()),
  });
  const chips = h("div", { class: "sc-chips" });
  const toolbar = h("div", { class: "sc-toolbar" }, h("label", { class: "sc-search" }, icon("search"), search), chips);
  const count = h("span");
  const list = h("div", { class: "wf-card" });

  page.append(
    h(
      "div",
      { class: "sc-hero" },
      h("div", { class: "sc-logo" }, icon("key")),
      h(
        "div",
        { class: "sc-hero-text" },
        h("h1", { class: "wf-h1" }, "Secrets"),
        h("div", { class: "wf-muted" }, "API keys, environment variables and logins for your agents. They use them by name and never see the values."),
      ),
      pill,
      h("button", { class: "wf-button wf-primary", onclick: () => openEditor(null) }, icon("plus"), "Add secret"),
    ),
    problem,
    h("div", { class: "sc-section" }, h("div", { class: "sc-label" }, "Stored", count), toolbar, list),
    h(
      "div",
      { class: "sc-section" },
      h("div", { class: "sc-label" }, "How agents use them"),
      h(
        "div",
        { class: "wf-card" },
        h(
          "div",
          { class: "sc-tools" },
          TOOLS.map((tool) => h("div", { class: "sc-tool" }, h("div", { class: "sc-tool-head" }, icon(tool.icon), h("span", { class: "sc-code" }, tool.name)), h("p", {}, tool.text))),
        ),
        h(
          "div",
          { class: "sc-note" },
          icon("alert"),
          "Redaction catches values a command prints, not a command written to disguise them. Keep “Ask before agents use it” on for anything important: you see each command first.",
        ),
      ),
    ),
  );

  const matches = (s) => [s.key, s.meta.description, s.meta.username, KINDS[s.meta.kind]].join(" ").toLowerCase().includes(query.toLowerCase());

  /** Redraws what changes (the search box stays, so it keeps focus). */
  function render() {
    problem.replaceChildren(icon("alert"), error);
    problem.style.display = error ? "" : "none";
    pill.replaceChildren(icon("lock"), secrets.length ? `${secrets.length} in your keychain` : "System keychain");
    count.textContent = secrets.length ? String(secrets.length) : "";
    toolbar.style.display = secrets.length ? "" : "none";

    const kinds = Object.keys(KIND_UI).filter((k) => secrets.some((s) => kindOf(s) === k));
    if (filter !== "all" && !kinds.includes(filter)) filter = "all";
    const chip = (id, label, n) => h("button", { class: `sc-chip${filter === id ? " sc-on" : ""}`, onclick: () => ((filter = id), render()) }, label, h("b", {}, String(n)));
    chips.replaceChildren(...(kinds.length > 1 ? [chip("all", "All", secrets.length), ...kinds.map((k) => chip(k, KIND_UI[k].short, secrets.filter((s) => kindOf(s) === k).length))] : []));

    const shown = secrets.filter((s) => (filter === "all" || kindOf(s) === filter) && matches(s));
    list.replaceChildren(
      ...(secrets.length === 0 ? [empty()] : shown.length === 0 ? [h("div", { class: "sc-nomatch" }, query ? `Nothing matches “${query}”.` : "Nothing here.")] : shown.map(row)),
    );
  }

  function openEditor(secret, initialKind = "env") {
    const editing = !!secret;
    const meta = secret?.meta ?? {};
    let kind = secret ? kindOf(secret) : initialKind;
    const key = h("input", {
      class: "sc-mono",
      value: secret?.key ?? "",
      placeholder: KIND_UI[kind].placeholder,
      disabled: editing,
      spellcheck: "false",
      oninput: (e) => (e.target.value = keyName(e.target.value)),
    });
    const value = h("input", { class: "sc-mono", type: "password", autocomplete: "off", spellcheck: "false", placeholder: editing ? "Leave empty to keep the current value" : "Paste the value" });
    const username = h("input", { value: meta.username ?? "", placeholder: "you@example.com", spellcheck: "false" });
    const description = h("input", { value: meta.description ?? "", placeholder: "e.g. Staging deploy token" });
    const approveBox = h("input", { type: "checkbox", class: "wf-switch", checked: meta.approve !== false, onchange: () => approveRow.classList.toggle("sc-off", !approveBox.checked) });
    const problem = h("div", { class: "sc-alert", style: "display:none" });
    const showValue = h("button", {
      class: "sc-icon-btn",
      type: "button",
      title: "Show value",
      onclick: () => {
        const show = value.type === "password";
        value.type = show ? "text" : "password";
        showValue.replaceChildren(icon(show ? "eyeOff" : "eye"));
        value.focus();
      },
    }, icon("eye"));

    const usernameField = h("label", { class: "sc-field" }, h("span", {}, "Username ", h("small", {}, "· agents can see this")), h("div", { class: "sc-input" }, username));
    const valueLabel = h("span");
    const kindButtons = h("div", { class: "sc-kinds", role: "radiogroup" });
    const approveRow = h(
      "label",
      { class: `sc-approve${meta.approve === false ? " sc-off" : ""}` },
      icon("shield"),
      h("div", {}, h("b", {}, "Ask before agents use it"), h("span", {}, "You see each command or page first and can decline.")),
      approveBox,
    );

    function setKind(next) {
      kind = next;
      usernameField.style.display = kind === "login" ? "" : "none";
      valueLabel.textContent = KIND_UI[kind].value;
      if (!editing) key.placeholder = KIND_UI[kind].placeholder;
      kindButtons.replaceChildren(
        ...Object.keys(KIND_UI).map((id) =>
          h("button", { type: "button", role: "radio", "aria-checked": String(id === kind), class: `sc-kind-opt${id === kind ? " sc-on" : ""}`, onclick: () => setKind(id) }, kindTile(id), KIND_UI[id].short),
        ),
      );
    }
    setKind(kind);

    const close = () => backdrop.remove();
    const save = async (e) => {
      e.preventDefault();
      problem.style.display = "none";
      try {
        if (!key.value) throw new Error("Give the secret a name.");
        if (!editing && secrets.some((s) => s.key === key.value)) throw new Error(`${key.value} already exists.`);
        await api.vault.set(key.value, value.value === "" ? undefined : value.value, {
          kind,
          description: description.value.trim(),
          username: kind === "login" ? username.value.trim() : "",
          approve: approveBox.checked,
        });
        revealed.delete(key.value);
        close();
        await refresh();
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
        { class: "wf-dialog sc-dialog", onsubmit: save, onkeydown: (e) => e.key === "Escape" && (e.stopPropagation(), close()) },
        h(
          "div",
          { class: "sc-dialog-head" },
          kindTile(editing ? kind : "token"),
          h("div", { class: "sc-dialog-title" }, h("h2", { class: "wf-h2", style: "font-weight:600" }, editing ? `Edit ${secret.key}` : "Add secret"), h("div", { class: "wf-subtle" }, "The value goes straight into your system keychain.")),
        ),
        h(
          "div",
          { class: "sc-dialog-body" },
          h("div", { class: "sc-field" }, "Kind", kindButtons),
          h("label", { class: "sc-field" }, h("span", {}, "Name ", h("small", {}, "· agents use it as an environment variable")), h("div", { class: `sc-input${editing ? " sc-locked" : ""}` }, h("span", {}, "$"), key)),
          usernameField,
          h("label", { class: "sc-field" }, valueLabel, h("div", { class: "sc-input" }, icon("lock"), value, showValue)),
          h("label", { class: "sc-field" }, h("span", {}, "Description ", h("small", {}, "· optional, agents can see this")), h("div", { class: "sc-input" }, description)),
          approveRow,
          problem,
        ),
        h(
          "div",
          { class: "sc-dialog-foot" },
          h("button", { class: "wf-button wf-ghost", type: "button", onclick: close }, "Cancel"),
          h("button", { class: "wf-button wf-primary", type: "submit" }, editing ? "Save changes" : "Add secret"),
        ),
      ),
    );
    document.body.append(backdrop);
    (editing ? value : key).focus();
  }

  render();
  void refresh();
  return () => {
    revealed.clear();
    document.querySelectorAll(".wf-dialog-backdrop").forEach((d) => d.remove());
    page.remove();
    style.remove();
  };
}
