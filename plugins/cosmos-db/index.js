// Cosmos DB for NoSQL. Secrets never enter agent arguments or command lines.
const WINDOWS = typeof navigator !== "undefined" && /Win/i.test(navigator.platform ?? navigator.userAgent);
const WRITES = new Set(["create", "upsert", "replace", "delete"]);
const encode = (value) => btoa(Array.from(new TextEncoder().encode(value), (b) => String.fromCharCode(b)).join(""));
const psQuote = (value) => `'${value.replace(/'/g, "''")}'`;
const shQuote = (value) => `'${value.replace(/'/g, "'\\''")}'`;
const powershell = (script) => {
  const bytes = new Uint8Array(script.length * 2);
  const view = new DataView(bytes.buffer);
  for (let i = 0; i < script.length; i++) view.setUint16(i * 2, script.charCodeAt(i), true);
  return `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${btoa(Array.from(bytes, (b) => String.fromCharCode(b)).join(""))}`;
};

export function validateRequest(args) {
  const request = JSON.parse(JSON.stringify(args)); // Snapshot exactly what is approved.
  if (!["databases", "containers", "query", "read", ...WRITES].includes(request.action)) throw new Error("Unknown Cosmos action.");
  for (const name of ["database", ...(!["databases", "containers"].includes(request.action) ? ["container"] : [])]) {
    if (request.action !== "databases" && (typeof request[name] !== "string" || !request[name].trim())) throw new Error(`${name} is required.`);
  }
  if (["read", "delete", "replace"].includes(request.action) && (typeof request.id !== "string" || !request.id)) throw new Error("id is required.");
  if (["read", "delete"].includes(request.action) && !("partition_key" in request)) throw new Error("partition_key is required (null is allowed).");
  if (["create", "upsert", "replace"].includes(request.action)) {
    if (!request.item || Array.isArray(request.item) || typeof request.item !== "object" || typeof request.item.id !== "string" || !request.item.id) throw new Error("item must be an object with a nonempty string id.");
    if (request.action === "replace" && request.item.id !== request.id) throw new Error("item.id must match id.");
  }
  if (request.action === "query" && (typeof request.query !== "string" || !/^\s*select\b/i.test(request.query))) throw new Error("Only SELECT queries are supported.");
  if (request.max_items !== undefined && (!Number.isInteger(request.max_items) || request.max_items < 1 || request.max_items > 100)) throw new Error("max_items must be 1–100.");
  if (JSON.stringify(request).length > 12000) throw new Error("Request exceeds 12 KB.");
  return request;
}

export function buildCommand(dir, request, windows = WINDOWS) {
  const payload = encode(JSON.stringify(request));
  const file = `${dir}/bridge.py`;
  if (!windows) return `python3 ${shQuote(file)} ${shQuote(payload)}`;
  const script = `& python ${psQuote(file)} ${psQuote(payload)}; exit $LASTEXITCODE`;
  return powershell(script);
}

export function activate(api) {
  const connections = async () => (await api.vault.list()).filter((c) => c.meta?.kind === "cosmos");
  async function find(name) {
    const all = await connections();
    const matches = all.filter((c) => c.key === name || c.meta.name === name);
    if (matches.length !== 1) throw new Error("Unknown or ambiguous connection. Use cosmos_list; add connections on the Cosmos DB page.");
    return matches[0];
  }
  async function run(args, caller) {
    const request = validateRequest(args);
    const conn = await find(request.connection);
    if (WRITES.has(request.action) && conn.meta.approveWrites !== false) {
      if (!(await api.confirm(`${caller?.label ?? "Agent"} in “${caller?.workspaceName ?? "workspace"}” requests a Cosmos DB write on ${conn.meta.name}:\n\n${JSON.stringify(request, null, 2)}`, { title: "Allow Cosmos DB write?", okLabel: "Write", kind: "warning", caller }))) throw new Error("The user declined this write.");
    }
    // Re-read after the approval wait: removal or credential edits invalidate the approval.
    const current = await find(conn.key);
    if (current.updatedAt !== conn.updatedAt || JSON.stringify(current.meta) !== JSON.stringify(conn.meta)) throw new Error("Connection changed; retry for fresh approval.");
    const payload = { ...request, auth: conn.meta.auth, endpoint: conn.meta.endpoint, key: conn.key };
    let command = buildCommand(api.dir, payload);
    // cmd.exe caps command lines at 8191 characters. Stage larger (non-credential)
    // requests in a self-removing script, using the same pattern as Databases.
    if (WINDOWS && command.length > 7000) {
      const file = `${api.dir}/.request-${crypto.randomUUID()}.ps1`;
      await api.writeFile(file, `try { & python ${psQuote(`${api.dir}/bridge.py`)} ${psQuote(encode(JSON.stringify(payload)))}; $result = $LASTEXITCODE } finally { Remove-Item -LiteralPath ${psQuote(file)} -Force }; exit $result`);
      command = powershell(`& ${psQuote(file)}; exit $LASTEXITCODE`);
    }
    const result = await api.vault.exec({ command, cwd: "", keys: conn.meta.auth === "connection-string" ? [conn.key] : [], timeoutSec: 60 });
    if (result.timedOut) throw new Error("Cosmos request timed out. A write may have committed; read before retrying.");
    if (result.code !== 0) throw new Error("Cosmos bridge failed. Install Python and run: python -m pip install azure-cosmos azure-identity (python3 on macOS/Linux).");
    let response;
    try { response = JSON.parse(result.output.trim()); } catch { throw new Error("Invalid bridge response; no raw output is exposed."); }
    if (response.error) throw new Error(response.error);
    return JSON.stringify(response);
  }
  const properties = {
    connection: { type: "string", description: "Name or key from cosmos_list" },
    database: { type: "string" }, container: { type: "string" },
    id: { type: "string" }, partition_key: { description: "Exact partition key value, null, or array for hierarchical keys" },
    max_items: { type: "integer", minimum: 1, maximum: 100, description: "Page size hint, default 25; lists return at most 100" },
  };
  api.registerAgentTool({ name: "cosmos_list", description: "List configured Cosmos DB connections without credentials, including write approval policy.", inputSchema: { type: "object", properties: {} }, run: async () => JSON.stringify((await connections()).map((c) => ({ key: c.key, name: c.meta.name, auth: c.meta.auth, approveWrites: c.meta.approveWrites !== false }))) });
  api.registerAgentTool({ name: "cosmos_read", description: "Read Cosmos DB for NoSQL: list databases/containers, SELECT query or read an item. Query returns one page and continuation; use parameters, projections and small page sizes. Database/container lists are capped at 100. Never performs writes.", inputSchema: { type: "object", properties: { ...properties, action: { type: "string", enum: ["databases", "containers", "query", "read"] }, query: { type: "string" }, parameters: { type: "array", items: { type: "object", properties: { name: { type: "string" }, value: {} }, required: ["name", "value"] } }, continuation: { type: "string" } }, required: ["connection", "action"] }, run: (args, caller) => { if (WRITES.has(args.action)) throw new Error("Writes must use cosmos_write."); return run(args, caller); } });
  api.registerAgentTool({ name: "cosmos_write", description: "Create, upsert (may overwrite), replace or delete ONE Cosmos DB item. User approval is required unless disabled on that connection. Include item.id and partition-key fields for writes; read/delete require explicit partition_key. No schema, stored procedure or bulk writes. After a timeout, read before retrying.", inputSchema: { type: "object", properties: { ...properties, action: { type: "string", enum: [...WRITES] }, item: { type: "object" } }, required: ["connection", "action", "database", "container"] }, run: (args, caller) => { if (!WRITES.has(args.action)) throw new Error("Use cosmos_read for reads."); return run(args, caller); } });
  api.registerPage({ id: "cosmos", title: "Cosmos DB", icon: "database", mount: (el) => mountPage(el, api, connections) });
  api.registerTool({ id: "open", title: "Cosmos DB", description: "Manage Cosmos DB connections and write permissions", palette: true, run: () => api.openPage("cosmos") });
}

function element(tag, text, className) {
  const el = document.createElement(tag);
  if (text) el.textContent = text;
  if (className) el.className = className;
  return el;
}

const PAGE_CSS = `
.cx-page { --cx-bg:var(--color-panel,#121215); --cx-card:var(--color-panel-raised,#17171b); --cx-fg:var(--color-fg,#e6e6ea); --cx-muted:var(--color-fg-muted,#9a9aa3); --cx-border:var(--color-border,#26262c); --cx-accent:var(--color-accent,#6d7cff); color:var(--cx-fg); font:13px/1.5 var(--font-sans,system-ui,sans-serif); max-width:900px; margin:0 auto; padding:32px; display:flex; flex-direction:column; gap:26px; container-type:inline-size; }
.cx-page *, .cx-page *::before, .cx-page *::after { box-sizing:border-box; }
.cx-page [hidden] { display:none!important; }
.cx-page h1,.cx-page h2,.cx-page p { margin:0; }
.cx-page h1 { font-size:21px; font-weight:650; letter-spacing:-.5px; }
.cx-page h2 { font-size:14px; font-weight:600; }
.cx-page button,.cx-page input { font:inherit; }
.cx-page button { cursor:pointer; }
.cx-page button:disabled,.cx-page input:disabled { opacity:.5; cursor:wait; }
.cx-page :focus-visible { outline:2px solid var(--cx-accent); outline-offset:3px; }
.cx-header { display:flex; align-items:center; gap:14px; }
.cx-logo { width:46px; height:46px; border-radius:13px; display:grid; place-items:center; background:linear-gradient(145deg,#26355b,#192137); border:1px solid #3a4d74; color:#a6c6ff; flex-shrink:0; }
.cx-icon { width:22px; height:22px; flex-shrink:0; }
.cx-subtitle { color:var(--cx-muted); font-size:12px; margin-top:3px!important; }
.cx-badge { font-size:10px; font-weight:600; letter-spacing:.06em; border:1px solid var(--cx-border); background:var(--cx-card); border-radius:5px; padding:3px 7px; color:var(--cx-muted); white-space:nowrap; }
.cx-header .cx-badge { margin-left:auto; }
.cx-section-head { display:flex; align-items:center; gap:8px; margin-bottom:10px; }
.cx-count { font-size:11px; padding:0 6px; border:1px solid var(--cx-border); border-radius:4px; color:var(--cx-muted); }
.cx-empty { display:flex; align-items:center; gap:13px; border:1px dashed var(--cx-border); border-radius:10px; padding:18px 20px; color:var(--cx-muted); font-size:12px; }
.cx-empty .cx-icon { opacity:.6; }
.cx-empty strong { display:block; font-size:12px; font-weight:500; color:var(--cx-fg); margin-bottom:2px; }
.cx-card { background:var(--cx-card); border:1px solid var(--cx-border); border-radius:12px; overflow:hidden; }
.cx-card-head { padding:20px 24px 17px; border-bottom:1px solid var(--cx-border); }
.cx-form-body { display:flex; flex-direction:column; gap:20px; padding:22px 24px; }
.cx-field { display:flex; flex-direction:column; gap:7px; min-width:0; font-size:12px; font-weight:500; }
.cx-hint { color:var(--cx-muted); font-size:11px; font-weight:400; }
.cx-page .cx-input { width:100%; min-width:0; height:38px; padding:0 11px; border:1px solid var(--color-border-strong,#33333b); border-radius:7px; background:var(--cx-bg); color:var(--cx-fg); outline:none; font-size:12px; transition:border-color .15s,box-shadow .15s; user-select:text; }
.cx-page .cx-input:focus { border-color:var(--cx-accent); box-shadow:0 0 0 3px color-mix(in srgb,var(--cx-accent) 12%,transparent); }
.cx-page .cx-input::placeholder { color:var(--color-fg-subtle,#6b6b74); }
.cx-mono { font-family:var(--font-mono,ui-monospace,monospace)!important; font-size:11px!important; }
.cx-auth { display:grid; grid-template-columns:1fr 1fr; gap:10px; }
.cx-page .cx-auth-button { display:flex; align-items:center; gap:10px; text-align:left; color:var(--cx-muted); background:var(--cx-bg); border:1px solid var(--cx-border); border-radius:8px; padding:12px; font-size:12px; }
.cx-auth-button strong { display:block; font-weight:500; }
.cx-auth-button small { display:block; font-size:10px; font-weight:400; color:var(--cx-muted); margin-top:2px; }
.cx-auth-button .cx-icon { width:18px; height:18px; }
.cx-page .cx-auth-button[aria-pressed=true] { color:var(--cx-fg); border-color:var(--cx-accent); background:color-mix(in srgb,var(--cx-accent) 7%,var(--cx-bg)); }
.cx-auth-button[aria-pressed=true] .cx-icon { color:var(--cx-accent); }
.cx-secret { display:flex; gap:8px; }
.cx-button { display:inline-flex; align-items:center; justify-content:center; gap:7px; padding:0 12px; height:34px; border:1px solid var(--color-border-strong,#33333b); border-radius:7px; background:transparent; color:var(--cx-fg); font-size:12px!important; white-space:nowrap; }
.cx-button:hover:not(:disabled) { background:var(--color-hover,#1d1d22); }
.cx-primary { background:var(--cx-accent); border-color:var(--cx-accent); color:var(--color-on-accent,#fff); font-weight:550!important; }
.cx-primary:hover:not(:disabled) { background:var(--cx-accent); filter:brightness(1.1); }
.cx-button .cx-icon { width:14px; height:14px; }
.cx-secret .cx-button { height:38px; color:var(--cx-muted); }
.cx-policy { display:flex; align-items:center; gap:12px; border:1px solid var(--cx-border); border-radius:8px; padding:14px; cursor:pointer; }
.cx-policy>.cx-icon { color:var(--cx-accent); width:19px; height:19px; }
.cx-policy-copy { flex:1; min-width:0; }
.cx-policy-copy strong { display:block; font-size:12px; font-weight:500; }
.cx-policy-copy small { display:block; margin-top:3px; color:var(--cx-muted); font-size:11px; font-weight:400; }
.cx-page .cx-switch { appearance:none; position:relative; width:32px; height:18px; margin:0; border:0; border-radius:10px; background:var(--color-active,#24242a); box-shadow:inset 0 0 0 1px var(--color-border-strong,#33333b); flex-shrink:0; cursor:pointer; transition:background .15s; }
.cx-switch::after { content:""; position:absolute; top:3px; left:3px; width:12px; height:12px; border-radius:50%; background:#fff; transition:transform .15s; }
.cx-page .cx-switch:checked { background:var(--cx-accent); box-shadow:none; }
.cx-switch:checked::after { transform:translateX(14px); }
.cx-policy:has(input:not(:checked)) { border-color:color-mix(in srgb,var(--color-warning,#f5b544) 45%,var(--cx-border)); }
.cx-policy:has(input:not(:checked))>.cx-icon { color:var(--color-warning,#f5b544); }
.cx-footer { border-top:1px solid var(--cx-border); display:flex; align-items:center; justify-content:space-between; gap:16px; padding:16px 24px; }
.cx-footnote { display:flex; align-items:center; gap:6px; font-size:11px; color:var(--cx-muted); }
.cx-footnote .cx-icon { width:13px; height:13px; }
.cx-message { color:var(--color-danger,#f26d6d); font-size:12px; padding:0 24px 16px; overflow-wrap:anywhere; }
.cx-message:empty { display:none; }
.cx-message[data-success=true] { color:var(--color-success,#4ade80); }
.cx-help { color:var(--cx-muted); font-size:12px; }
.cx-help summary { cursor:pointer; font-size:12px; width:fit-content; }
.cx-help-body { padding:14px 0 0; display:flex; flex-direction:column; gap:10px; line-height:1.7; }
.cx-command { display:block; background:var(--cx-bg); border:1px solid var(--cx-border); border-radius:7px; padding:10px 12px; overflow-wrap:anywhere; user-select:text; }
.cx-connection { display:flex; align-items:center; gap:12px; padding:16px; flex-wrap:wrap; }
.cx-connection + .cx-connection { border-top:1px solid var(--cx-border); }
.cx-connection-info { flex:1; min-width:140px; overflow-wrap:anywhere; }
.cx-connection-info strong { font-size:12px; font-weight:550; }
.cx-connection-info small { display:block; color:var(--cx-muted); font-size:11px; margin-top:3px; }
.cx-inline-policy { display:flex; align-items:center; gap:10px; color:var(--cx-muted); font-size:11px; cursor:pointer; }
.cx-remove { border-color:transparent; color:var(--cx-muted); font-size:11px!important; }
.cx-remove:hover:not(:disabled) { color:var(--color-danger,#f26d6d); }
@container (max-width:480px) { .cx-auth { grid-template-columns:1fr; } .cx-footer { align-items:flex-start; flex-direction:column; } .cx-footer .cx-primary { width:100%; } .cx-header .cx-badge { display:none; } .cx-card-head,.cx-form-body { padding:18px; } }
@media (max-width:600px) { .cx-page { padding:20px 16px; } }
`;

function icon(kind) {
  const paths = {
    database: '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>',
    shield: '<path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z"/><path d="m8 12 3 3 5-6"/>',
    key: '<circle cx="8" cy="8" r="5"/><path d="m12 12 9 9m-4-4 3-3m-6 0 3-3"/>',
    microsoft: '<path d="M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    lock: '<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>',
  };
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24"); svg.setAttribute("fill", "none"); svg.setAttribute("stroke", "currentColor"); svg.setAttribute("stroke-width", "1.5"); svg.setAttribute("stroke-linecap", "round"); svg.setAttribute("stroke-linejoin", "round"); svg.setAttribute("aria-hidden", "true"); svg.classList.add("cx-icon");
  svg.innerHTML = paths[kind] ?? paths.database; // Static, trusted SVG only.
  return svg;
}

function mountPage(root, api, connections) {
  let disposed = false;
  const page = element("div", null, "cx-page");
  const style = element("style", PAGE_CSS);
  const header = element("header", null, "cx-header");
  const logo = element("div", null, "cx-logo"); logo.append(icon("database"));
  const heading = element("div"); heading.append(element("h1", "Cosmos DB"), element("p", "Your Azure data, connected to your agents.", "cx-subtitle"));
  header.append(logo, heading, element("span", "AZURE · NOSQL", "cx-badge"));
  const section = element("section");
  const sectionHead = element("div", null, "cx-section-head");
  const count = element("span", "0", "cx-count"); sectionHead.append(element("h2", "Connections"), count);
  const list = element("div");
  section.append(sectionHead, list);
  const form = element("form", null, "cx-card");
  const formHead = element("div", null, "cx-card-head");
  formHead.append(element("h2", "Add a connection"), element("p", "Choose how to sign in. Your agents only see the connection name.", "cx-subtitle"));
  const body = element("div", null, "cx-form-body");
  const field = (title, input, hint) => { const label = element("label", null, "cx-field"); label.append(element("span", title), input); if (hint) label.append(element("span", hint, "cx-hint")); return label; };
  const name = element("input", null, "cx-input"); name.required = true; name.placeholder = "e.g. Production, Staging, My project";
  const auth = element("select"); auth.hidden = true;
  for (const [value, title] of [["connection-string", "Connection string"], ["entra", "Microsoft integrated (Entra identity)"]]) { const o = element("option", title); o.value = value; auth.append(o); }
  const authGroup = element("div", null, "cx-field"); authGroup.append(element("span", "Authentication"));
  const authChoices = element("div", null, "cx-auth"); authGroup.append(authChoices);
  const authButtons = [];
  for (const [value, title, subtitle, glyph] of [["connection-string", "Connection string", "Account endpoint & access key", "key"], ["entra", "Microsoft identity", "Your Azure sign-in · no key needed", "microsoft"]]) {
    const button = element("button", null, "cx-auth-button"); button.type = "button";
    const text = element("span"); text.append(element("strong", title), element("small", subtitle)); button.append(icon(glyph), text);
    button.onclick = () => { auth.value = value; changeAuth(); }; authButtons.push([value, button]); authChoices.append(button);
  }
  const secret = element("input", null, "cx-input cx-mono"); secret.type = "password"; secret.autocomplete = "off"; secret.spellcheck = false; secret.placeholder = "AccountEndpoint=https://…;AccountKey=…;";
  const show = element("button", "Show", "cx-button"); show.type = "button"; show.setAttribute("aria-label", "Show connection string");
  show.onclick = () => { const visible = secret.type === "password"; secret.type = visible ? "text" : "password"; show.textContent = visible ? "Hide" : "Show"; show.setAttribute("aria-label", `${visible ? "Hide" : "Show"} connection string`); };
  const secretRow = element("div", null, "cx-secret"); secretRow.append(secret, show);
  const secretField = element("div", null, "cx-field"); const secretLabel = element("label", "Connection string");
  secret.id = `cx-secret-${crypto.randomUUID()}`; secretLabel.htmlFor = secret.id;
  secretField.append(secretLabel, secretRow, element("span", "Find this in Azure Portal → your Cosmos DB account → Keys.", "cx-hint"));
  const endpoint = element("input", null, "cx-input cx-mono"); endpoint.type = "url"; endpoint.placeholder = "https://your-account.documents.azure.com:443/";
  const endpointField = field("Account endpoint", endpoint, "Uses your Azure CLI sign-in or configured managed / workload identity.");
  const approve = element("input", null, "cx-switch"); approve.type = "checkbox"; approve.checked = true;
  const label = element("label", null, "cx-policy");
  const policyCopy = element("div", null, "cx-policy-copy"); const policyHint = element("small", "Review and approve each create, update or delete before it runs.");
  policyCopy.append(element("strong", "Ask before agents write"), policyHint); label.append(icon("shield"), policyCopy, approve);
  approve.onchange = () => { policyHint.textContent = approve.checked ? "Review and approve each create, update or delete before it runs." : "Agents can overwrite and delete items without asking. Use with care."; };
  const message = element("div", null, "cx-message"); message.setAttribute("role", "status"); message.setAttribute("aria-live", "polite");
  const save = element("button", null, "cx-button cx-primary"); save.append(icon("plus"), document.createTextNode("Add connection")); save.type = "submit";
  const changeAuth = () => { secretField.hidden = auth.value !== "connection-string"; endpointField.hidden = auth.value !== "entra"; secret.required = !secretField.hidden; endpoint.required = !endpointField.hidden; for (const [value, button] of authButtons) button.setAttribute("aria-pressed", String(auth.value === value)); };
  auth.onchange = changeAuth; changeAuth();
  name.setAttribute("aria-label", "Connection name");
  auth.setAttribute("aria-label", "Authentication method");
  secret.setAttribute("aria-label", "Connection string");
  endpoint.setAttribute("aria-label", "Account endpoint");
  body.append(field("Connection name", name), auth, authGroup, secretField, endpointField, label);
  const footer = element("div", null, "cx-footer"); const secureNote = element("span", null, "cx-footnote"); secureNote.append(icon("lock"), document.createTextNode("Credentials stay in your system keychain")); footer.append(secureNote, save);
  form.append(formHead, body, footer, message);
  async function refresh() {
    const all = await connections();
    if (disposed) return;
    list.replaceChildren();
    count.textContent = String(all.length);
    list.className = all.length ? "cx-card" : "";
    if (!all.length) {
      const empty = element("div", null, "cx-empty"); const copy = element("div"); copy.append(element("strong", "No connections yet"), document.createTextNode("Add an account below to let agents browse, query and write your data.")); empty.append(icon("database"), copy); list.append(empty);
    }
    for (const conn of all) {
      const row = element("div", null, "cx-connection");
      const toggle = element("input", null, "cx-switch"); toggle.type = "checkbox"; toggle.checked = conn.meta.approveWrites !== false;
      const policy = element("label", "Ask before writes", "cx-inline-policy"); policy.append(toggle); toggle.setAttribute("aria-label", `Ask before writes to ${conn.meta.name}`);
      toggle.onchange = async () => {
        toggle.disabled = true;
        try {
          if (!toggle.checked && !(await api.confirm(`Allow agents to write to ${conn.meta.name} without asking? They can overwrite and delete items.`, { title: "Disable write approval?", okLabel: "Allow writes", kind: "warning" }))) { toggle.checked = true; return; }
          await api.vault.set(conn.key, undefined, { ...conn.meta, approveWrites: toggle.checked });
          conn.meta.approveWrites = toggle.checked;
        } catch (err) { toggle.checked = conn.meta.approveWrites !== false; message.dataset.success = "false"; message.textContent = String(err.message); }
        finally { toggle.disabled = false; }
      };
      const remove = element("button", "Remove", "cx-button cx-remove"); remove.setAttribute("aria-label", `Remove ${conn.meta.name}`);
      remove.onclick = async () => { try { if (await api.confirm(`Remove ${conn.meta.name} and its saved credentials? The database is not deleted.`, { title: "Remove Cosmos connection?", okLabel: "Remove" })) { await api.vault.delete(conn.key); await refresh(); } } catch (err) { message.dataset.success = "false"; message.textContent = String(err.message); } };
      const info = element("div", null, "cx-connection-info");
      info.append(element("strong", conn.meta.name), element("small", `${conn.meta.auth === "entra" ? "Microsoft identity" : "Connection string"} · ${conn.meta.endpoint || "Azure Cosmos DB"}`));
      row.append(icon("database"), info, policy, remove); list.append(row);
    }
  }
  form.onsubmit = async (event) => {
    event.preventDefault(); save.disabled = true; message.textContent = ""; message.dataset.success = "false";
    try {
      const title = name.value.trim();
      if (!title || (await connections()).some((c) => c.meta.name.toLowerCase() === title.toLowerCase())) throw new Error("Choose a unique connection name.");
      let url;
      if (auth.value === "connection-string") {
        const match = secret.value.match(/(?:^|;)\s*AccountEndpoint\s*=\s*([^;]+)/i);
        if (!match || !/(?:^|;)\s*AccountKey\s*=\s*[^;]+/i.test(secret.value)) throw new Error("A connection string with AccountEndpoint and AccountKey is required.");
        url = new URL(match[1].trim());
      } else url = new URL(endpoint.value);
      if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash || url.pathname !== "/") throw new Error("Use an HTTPS account endpoint without credentials, query or path.");
      if (!approve.checked && !(await api.confirm(`Allow agents to overwrite and delete items on ${title} without asking?`, { title: "Disable write approval?", okLabel: "Allow writes", kind: "warning" }))) return;
      await api.vault.set(`COSMOS_${crypto.randomUUID().replace(/-/g, "_").toUpperCase()}`, auth.value === "connection-string" ? secret.value : "", { kind: "cosmos", name: title, auth: auth.value, endpoint: url.origin, approveWrites: approve.checked });
      secret.value = ""; secret.type = "password"; show.textContent = "Show"; show.setAttribute("aria-label", "Show connection string"); name.value = ""; await refresh(); message.dataset.success = "true"; message.textContent = "Connection added. Your agents can now use it by name.";
    } catch (err) { message.textContent = String(err.message); }
    finally { save.disabled = false; }
  };
  const help = element("details", null, "cx-help"); help.append(element("summary", "Setup & authentication help"));
  const helpBody = element("div", null, "cx-help-body");
  helpBody.append(element("p", "Requires Python 3 and the Azure SDKs. Install once:"), element("code", "python -m pip install azure-cosmos azure-identity", "cx-command cx-mono"), element("p", "On macOS / Linux, use python3. Microsoft identity uses DefaultAzureCredential: run az login, or configure a managed / workload identity. Grant the identity Cosmos DB data-plane RBAC. This is Microsoft Entra authentication, not Windows SSPI."), element("p", "Supports the Cosmos DB for NoSQL API. Reads do not ask for approval; write permissions are controlled per connection."));
  help.append(helpBody);
  page.append(style, header, section, form, help); root.replaceChildren(page);
  refresh().catch((err) => { message.textContent = String(err.message); });
  return () => { disposed = true; page.remove(); };
}
