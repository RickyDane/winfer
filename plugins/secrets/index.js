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

  api.registerPage({ id: "secrets", title: "Secrets", icon: "key", mount: (el) => mountPage(el, api) });

  api.registerTool({
    id: "open",
    title: "Secrets",
    description: "Open the secrets page to add, change or remove stored API keys, passwords and environment variables",
    palette: [{ title: "Secrets: manage stored secrets" }],
    run: () => api.openPage("secrets"),
  });
}

function mountPage(root, api) {
  let secrets = [];
  let query = "";
  let error = "";
  /** Revealed values by key, cleared when the page closes. */
  const revealed = new Map();
  const page = h("div", { class: "wf-page" });
  root.append(page);

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

  function row(secret) {
    const value = revealed.get(secret.key);
    const meta = secret.meta;
    return h(
      "div",
      { class: "wf-row" },
      h(
        "div",
        { class: "wf-grow wf-stack", style: "gap:4px" },
        h(
          "div",
          { class: "wf-inline" },
          h("span", { class: "wf-mono", style: "font-size:13px" }, secret.key),
          h("span", { class: "wf-badge" }, KINDS[meta.kind] ?? "Secret"),
          needsApproval(secret) && h("span", { class: "wf-badge wf-accent", title: "Agents need your OK each time" }, "Asks first"),
        ),
        meta.username && h("div", { class: "wf-subtle" }, `Username: ${meta.username}`),
        meta.description && h("div", { class: "wf-muted" }, meta.description),
        value !== undefined && h("div", { class: "wf-mono wf-muted", style: "word-break:break-all;user-select:text" }, value),
      ),
      h(
        "div",
        { class: "wf-inline" },
        h(
          "button",
          {
            class: "wf-button wf-ghost",
            onclick: () =>
              act(async () => {
                if (revealed.has(secret.key)) revealed.delete(secret.key);
                else revealed.set(secret.key, await api.vault.reveal(secret.key));
              }),
          },
          value === undefined ? "Show" : "Hide",
        ),
        h(
          "button",
          {
            class: "wf-button wf-ghost",
            onclick: () =>
              act(async () => {
                await navigator.clipboard.writeText(await api.vault.reveal(secret.key));
                api.toast(`Copied ${secret.key}`, "success");
              }),
          },
          "Copy",
        ),
        h("button", { class: "wf-button", onclick: () => openEditor(secret) }, "Edit"),
        h(
          "button",
          {
            class: "wf-button wf-danger",
            onclick: () =>
              act(async () => {
                if (await api.confirm(`Delete ${secret.key}? Its value is removed from your keychain.`, { title: "Delete secret", okLabel: "Delete" })) {
                  revealed.delete(secret.key);
                  await api.vault.delete(secret.key);
                }
              }),
          },
          "Delete",
        ),
      ),
    );
  }

  const list = h("div", { class: "wf-card" });
  const problem = h("p", { class: "wf-error", style: "margin:0" });
  const search = h("input", {
    class: "wf-input",
    style: "width:220px",
    type: "search",
    placeholder: "Search secrets",
    oninput: (e) => {
      query = e.target.value;
      render();
    },
  });
  page.append(
    h(
      "div",
      { class: "wf-header" },
      h(
        "div",
        { class: "wf-stack", style: "gap:4px" },
        h("h1", { class: "wf-h1" }, "Secrets"),
        h(
          "p",
          { class: "wf-muted", style: "margin:0" },
          "Stored in your system keychain. Agents see only the names: they run commands with the secrets as environment variables or have winfer type them into the browser, and never get the values back.",
        ),
      ),
      h("div", { class: "wf-inline" }, search, h("button", { class: "wf-button wf-primary", onclick: () => openEditor(null) }, "Add secret")),
    ),
    problem,
    list,
    h(
      "div",
      { class: "wf-card" },
      h(
        "div",
        { class: "wf-row" },
        h(
          "div",
          { class: "wf-grow wf-stack", style: "gap:6px" },
          h("h2", { class: "wf-h2" }, "How agents use them"),
          h(
            "p",
            { class: "wf-muted", style: "margin:0" },
            "Agents started by winfer get three tools: ",
            h("span", { class: "wf-mono" }, "list_secrets"),
            ", ",
            h("span", { class: "wf-mono" }, "run_with_secrets"),
            " (a command gets the secrets as $NAME; their values are cut from its output) and ",
            h("span", { class: "wf-mono" }, "fill_secret_in_browser"),
            ".",
          ),
          h(
            "p",
            { class: "wf-subtle", style: "margin:0" },
            "Redaction catches values a command prints, not a command written to disguise them. Keep “Ask before agents use it” on for anything important: you see each command first.",
          ),
        ),
      ),
    ),
  );

  /** Redraws the list (the rest of the page stays, so the search keeps focus). */
  function render() {
    problem.textContent = error;
    search.style.display = secrets.length ? "" : "none";
    const shown = secrets.filter((s) =>
      [s.key, s.meta.description, s.meta.username, KINDS[s.meta.kind]].join(" ").toLowerCase().includes(query.toLowerCase()),
    );
    list.replaceChildren(
      ...(secrets.length === 0
        ? [h("div", { class: "wf-empty" }, "No secrets yet. Add an API key, an environment variable or a login.")]
        : shown.length === 0
          ? [h("div", { class: "wf-empty" }, `Nothing matches “${query}”.`)]
          : shown.map(row)),
    );
  }

  function openEditor(secret) {
    const editing = !!secret;
    const meta = secret?.meta ?? {};
    let kind = meta.kind ?? "env";
    const key = h("input", { class: "wf-input wf-mono", value: secret?.key ?? "", placeholder: "API_TOKEN", disabled: editing, oninput: (e) => (e.target.value = keyName(e.target.value)) });
    const value = h("input", { class: "wf-input wf-mono", type: "password", autocomplete: "off", placeholder: editing ? "Leave empty to keep the current value" : "Value" });
    const username = h("input", { class: "wf-input", value: meta.username ?? "", placeholder: "Username or email (agents can see this)" });
    const description = h("input", { class: "wf-input", value: meta.description ?? "", placeholder: "What it's for, e.g. “Staging deploy token” (agents can see this)" });
    const approveBox = h("input", { type: "checkbox", class: "wf-switch", checked: meta.approve !== false });
    const problem = h("p", { class: "wf-error", style: "margin:0" });
    const usernameField = h("label", { class: "wf-field" }, "Username", username);
    const kindSelect = h(
      "select",
      { class: "wf-select", onchange: (e) => { kind = e.target.value; usernameField.style.display = kind === "login" ? "" : "none"; } },
      Object.entries(KINDS).map(([id, label]) => h("option", { value: id, selected: id === kind }, label)),
    );
    usernameField.style.display = kind === "login" ? "" : "none";
    const show = h("button", { class: "wf-button wf-ghost", type: "button", onclick: () => { value.type = value.type === "password" ? "text" : "password"; show.textContent = value.type === "password" ? "Show" : "Hide"; } }, "Show");

    const close = () => backdrop.remove();
    const save = async (e) => {
      e.preventDefault();
      problem.textContent = "";
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
        problem.textContent = String(err?.message ?? err);
      }
    };
    const backdrop = h(
      "div",
      { class: "wf-dialog-backdrop", onmousedown: (e) => e.target === backdrop && close() },
      h(
        "form",
        { class: "wf-dialog", onsubmit: save, onkeydown: (e) => e.key === "Escape" && (e.stopPropagation(), close()) },
        h("h2", { class: "wf-h2" }, editing ? `Edit ${secret.key}` : "Add secret"),
        h("label", { class: "wf-field" }, "Name (agents use this, like $API_TOKEN)", key),
        h("label", { class: "wf-field" }, "Kind", kindSelect),
        usernameField,
        h("label", { class: "wf-field" }, kind === "login" ? "Password" : "Value", h("div", { class: "wf-inline", style: "flex-wrap:nowrap" }, h("div", { style: "flex:1;display:flex;flex-direction:column" }, value), show)),
        h("label", { class: "wf-field" }, "Description", description),
        h("label", { class: "wf-inline", style: "font-size:12px;color:var(--color-fg)" }, approveBox, "Ask before agents use it"),
        problem,
        h("div", { class: "wf-inline", style: "justify-content:flex-end" }, h("button", { class: "wf-button", type: "button", onclick: close }, "Cancel"), h("button", { class: "wf-button wf-primary", type: "submit" }, editing ? "Save" : "Add")),
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
  };
}
