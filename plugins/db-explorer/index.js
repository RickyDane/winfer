// DB Explorer: read-only access to the user's Postgres, MySQL and SQLite databases, for them on a
// page (connections, schema, a query editor, results and a history) and for agents as tools.
//
// Connections live in the plugin's vault: a Postgres URL or a MySQL password is a secret the
// command gets as an environment variable, so agents name a connection and never see it. The
// database CLIs (psql, mysql, sqlite3) run through `api.vault.exec`. Read-only is enforced twice:
// by the database session (Postgres `default_transaction_read_only`, MySQL `SET SESSION
// TRANSACTION READ ONLY`, `sqlite3 -readonly -safe`) and by `checkReadOnly`, which only lets one
// read statement through and refuses the CLIs' own commands (psql `\!`, mysql `system`, sqlite
// `.shell`), which would otherwise run on the user's machine.

const IS_WIN = typeof navigator !== "undefined" && (/Win/i.test(navigator.platform ?? "") || /Windows/i.test(navigator.userAgent));
const HISTORY = "winfer.db-explorer.history";
const MAX_HISTORY = 200;
const ROW_LIMITS = [100, 500, 1000, 5000];
/** What an agent sees of a result, at most. */
const AGENT_CHARS = 12_000;
const ROWS_MARK = "@@WINFER_ROWS";

export const KINDS = {
  postgres: { name: "PostgreSQL", short: "Postgres", cli: "psql", port: 5432, placeholder: "postgres://user:password@localhost:5432/app" },
  mysql: { name: "MySQL / MariaDB", short: "MySQL", cli: "mysql", port: 3306, placeholder: "mysql://user:password@localhost:3306/app" },
  sqlite: { name: "SQLite", short: "SQLite", cli: "sqlite3", port: null, placeholder: "data/app.db" },
};

/** Statements each dialect may start with: reads only. */
const READS = {
  postgres: ["select", "with", "explain", "show", "values", "table"],
  mysql: ["select", "with", "explain", "describe", "desc", "show", "values", "table"],
  sqlite: ["select", "with", "explain", "pragma", "values"],
};

/** Things a read statement could still use to write or to leave read-only mode. */
const FORBIDDEN = {
  postgres: /\b(set_config|default_transaction_read_only|transaction_read_only|dblink\w*|lo_import|lo_export|pg_terminate_backend|pg_cancel_backend|pg_reload_conf)\b/i,
  mysql: /\binto\s+(outfile|dumpfile)\b|\b(load_file|get_lock|release_lock)\s*\(/i,
  sqlite: /\b(load_extension|writefile|readfile|edit|fts3_tokenizer)\s*\(/i,
};

// SQL -------------------------------------------------------------------------------------------

/**
 * Blanks out strings, quoted names and comments (keeping newlines and length), so what's left is
 * the SQL's code: `;`, `\` and keywords in it are real. `open` is set when something isn't closed.
 */
export function blankSql(sql, dialect) {
  let code = "";
  let open = false;
  let i = 0;
  const n = sql.length;
  const blank = (s) => s.replace(/[^\n]/g, " ");
  while (i < n) {
    const c = sql[i];
    const next = sql[i + 1];
    if ((c === "-" && next === "-") || (dialect === "mysql" && c === "#")) {
      let j = sql.indexOf("\n", i);
      if (j < 0) j = n;
      code += blank(sql.slice(i, j));
      i = j;
      continue;
    }
    if (c === "/" && next === "*") {
      const j = sql.indexOf("*/", i + 2);
      if (j < 0) {
        open = true;
        code += blank(sql.slice(i));
        break;
      }
      code += blank(sql.slice(i, j + 2));
      i = j + 2;
      continue;
    }
    if (dialect === "postgres" && c === "$" && !/[\w$]/.test(sql[i - 1] ?? "")) {
      const tag = /^\$(?:[A-Za-z_][A-Za-z0-9_]*)?\$/.exec(sql.slice(i))?.[0];
      if (tag) {
        const j = sql.indexOf(tag, i + tag.length);
        if (j < 0) {
          open = true;
          code += blank(sql.slice(i));
          break;
        }
        code += tag + blank(sql.slice(i + tag.length, j)) + tag;
        i = j + tag.length;
        continue;
      }
    }
    if (c === "'" || c === '"' || (c === "`" && dialect !== "postgres")) {
      const eString = dialect === "postgres" && c === "'" && /[eE]/.test(sql[i - 1] ?? "") && !/[\w$]/.test(sql[i - 2] ?? "");
      const escapes = (dialect === "mysql" && c !== "`") || eString;
      let j = i + 1;
      let closed = false;
      while (j < n) {
        if (escapes && sql[j] === "\\") {
          j += 2;
          continue;
        }
        if (sql[j] === c) {
          if (sql[j + 1] === c) {
            j += 2;
            continue;
          }
          closed = true;
          break;
        }
        j++;
      }
      if (!closed) {
        open = true;
        code += c + blank(sql.slice(i + 1));
        break;
      }
      code += c + blank(sql.slice(i + 1, j)) + c;
      i = j + 1;
      continue;
    }
    code += c;
    i++;
  }
  return { code, open };
}

/** The statements of `sql` with where each starts and ends, split on real `;`. */
export function splitStatements(sql, dialect) {
  const { code } = blankSql(sql, dialect);
  const out = [];
  let start = 0;
  for (let i = 0; i <= code.length; i++) {
    if (i === code.length || code[i] === ";") {
      const text = sql.slice(start, i);
      if (code.slice(start, i).trim()) out.push({ start, end: i, text: text.trim() });
      start = i + 1;
    }
  }
  return out;
}

/** The statement the cursor is in (or the last one before it), for "run at cursor". */
export function statementAt(sql, dialect, pos) {
  const all = splitStatements(sql, dialect);
  if (all.length === 0) return "";
  const hit = all.find((s) => pos >= s.start && pos <= s.end + 1) ?? [...all].reverse().find((s) => s.end < pos) ?? all[0];
  return hit.text;
}

/** null if `sql` is one read statement this plugin will run; else why not. */
export function checkReadOnly(sql, dialect) {
  const { code, open } = blankSql(sql, dialect);
  if (open) return "The query has a string, quoted name or comment that isn't closed.";
  if (dialect !== "sqlite" && code.includes("\\")) return "Backslash commands (like psql's \\d or mysql's \\!) aren't allowed. Write plain SQL; use the schema tool to list tables.";
  if (dialect === "sqlite" && /^[ \t]*\./m.test(code)) return "sqlite3 dot commands (like .tables) aren't allowed. Write plain SQL; use the schema tool to list tables.";
  const statements = splitStatements(sql, dialect);
  if (statements.length === 0) return "There's no query to run.";
  if (statements.length > 1) return "Run one statement at a time.";
  const { code: stmt } = blankSql(statements[0].text, dialect);
  const word = /^[\s(]*([A-Za-z_]+)/.exec(stmt)?.[1]?.toLowerCase() ?? "";
  if (!READS[dialect].includes(word)) {
    return `${word ? word.toUpperCase() : "This statement"} isn't allowed: DB Explorer is read-only and runs only ${READS[dialect].map((w) => w.toUpperCase()).join(", ")}.`;
  }
  const bad = FORBIDDEN[dialect].exec(stmt);
  if (bad) return `${bad[0].trim()} isn't allowed: DB Explorer is read-only.`;
  return null;
}

/** The parts of a database URL: `scheme://user:password@host:port/database?params`. */
export function parseDbUrl(text) {
  const m = /^([a-z][\w+.-]*):\/\/(.*)$/i.exec(String(text).trim());
  if (!m) return null;
  const scheme = m[1].toLowerCase();
  const kind = /^postgres(ql)?$/.test(scheme) ? "postgres" : /^(mysql|mariadb)$/.test(scheme) ? "mysql" : null;
  const decode = (s) => {
    try {
      return decodeURIComponent(s);
    } catch {
      return s;
    }
  };
  let rest = m[2];
  const q = rest.search(/[?#]/);
  const params = q < 0 ? "" : rest.slice(q + 1).replace(/#.*$/, "");
  if (q >= 0) rest = rest.slice(0, q);
  const at = rest.lastIndexOf("@");
  const auth = at < 0 ? "" : rest.slice(0, at);
  const after = at < 0 ? rest : rest.slice(at + 1);
  const slash = after.indexOf("/");
  const hostPort = slash < 0 ? after : after.slice(0, slash);
  const database = slash < 0 ? "" : decode(after.slice(slash + 1));
  const hp = /^(\[[^\]]*\]|[^:]*)(?::(\d+))?$/.exec(hostPort) ?? [hostPort, hostPort, ""];
  const colon = auth.indexOf(":");
  return {
    kind,
    user: decode(colon < 0 ? auth : auth.slice(0, colon)),
    password: colon < 0 ? "" : decode(auth.slice(colon + 1)),
    host: hp[1].replace(/^\[|\]$/g, ""),
    port: hp[2] ? Number(hp[2]) : null,
    database,
    params,
  };
}

/** RFC 4180 CSV (psql --csv, sqlite3 -csv) into rows of cells. */
export function parseCsv(text) {
  const rows = [];
  let row = [];
  let cell = "";
  let quoted = false;
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        cell += '"';
        i += 2;
        continue;
      }
      if (c === '"') quoted = false;
      else cell += c;
      i++;
      continue;
    }
    if (c === '"' && cell === "") quoted = true;
    else if (c === ",") {
      row.push(cell);
      cell = "";
    } else if (c === "\n" || (c === "\r" && text[i + 1] === "\n")) {
      if (c === "\r") i++;
      row.push(cell);
      rows.push(row);
      row = [];
      cell = "";
    } else cell += c;
    i++;
  }
  if (cell !== "" || row.length) {
    row.push(cell);
    rows.push(row);
  }
  return rows;
}

/** mysql --batch output: tab-separated, with \t \n \\ \0 escaped inside values. */
export function parseTsv(text) {
  const unescape = (s) => s.replace(/\\([tn0\\])/g, (_, c) => ({ t: "\t", n: "\n", 0: "\0", "\\": "\\" })[c]);
  return text
    .split(/\r?\n/)
    .filter((line, i, all) => line !== "" || i < all.length - 1)
    .map((line) => line.split("\t").map(unescape));
}

const shq = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;
/** A PowerShell single-quoted string. */
const psq = (s) => `'${String(s).replace(/'/g, "''")}'`;
/** One Windows command-line argument (CommandLineToArgvW rules). */
const winArg = (s) => (/^[\w./:@=+-]+$/.test(s) ? s : `"${String(s).replace(/(\\*)"/g, '$1$1\\"').replace(/(\\+)$/, "$1$1")}"`);

function delimiter(sql) {
  let d = "WINFER_SQL";
  while (sql.includes(d)) d += "_X";
  return d;
}

/**
 * The shell command that runs `sql` on a connection read-only, prints the first `maxRows`
 * rows (plus the header), then `@@WINFER_ROWS <lines of output>`, and exits with the CLI's code.
 * `conn` is `{ key, meta }` from the vault.
 */
export function buildCommand(conn, sql, { maxRows = 200, timeoutSec = 60, win = IS_WIN } = {}) {
  const { key, meta } = conn;
  const ms = Math.max(1, timeoutSec - 2) * 1000;
  // [program, args, env] with `$KEY` standing for the secret, filled in per shell below.
  let program;
  let args;
  let env = [];
  if (meta.kind === "postgres") {
    program = "psql";
    args = [{ secret: key }, "-X", "-q", "--csv", "-v", "ON_ERROR_STOP=1", "-P", "null=NULL"];
    env = [
      ["PGOPTIONS", `-c default_transaction_read_only=on -c statement_timeout=${ms}`],
      ["PGCONNECT_TIMEOUT", "10"],
      ["PGAPPNAME", "winfer-db-explorer"],
    ];
  } else if (meta.kind === "mysql") {
    program = "mysql";
    args = ["--batch", "--default-character-set=utf8mb4", "--connect-timeout=10", "--init-command=SET SESSION TRANSACTION READ ONLY"];
    if (meta.host) args.push("-h", meta.host);
    if (meta.port) args.push("-P", String(meta.port));
    if (meta.user) args.push("-u", meta.user);
    if (meta.database) args.push(meta.database);
    if (meta.hasPassword) env.push(["MYSQL_PWD", { secret: key }]);
  } else if (meta.kind === "sqlite") {
    program = "sqlite3";
    args = ["-readonly", "-safe", "-bail", "-csv", "-header", "-nullvalue", "NULL", "-cmd", ".timeout 5000", meta.path];
  } else {
    throw new Error(`Unknown database kind “${meta.kind}”.`);
  }

  if (!win) {
    const arg = (a) => (typeof a === "object" ? `"$${a.secret}"` : shq(a));
    const envs = env.map(([k, v]) => `${k}=${arg(v)}`).join(" ");
    const d = delimiter(sql);
    return [
      "t=$(mktemp) || exit 1",
      `${envs ? `${envs} ` : ""}${program} ${args.map(arg).join(" ")} >"$t" 2>&1 <<'${d}'`,
      sql,
      d,
      "c=$?",
      `head -n ${maxRows + 1} "$t"`,
      `printf '\\n${ROWS_MARK} %s\\n' "$(wc -l < "$t" | tr -d ' ')"`,
      'rm -f "$t"',
      "exit $c",
    ].join("\n");
  }

  if (/^'@/m.test(sql)) throw new Error("A line of the query starts with '@, which can't be passed on Windows.");
  const psArg = (a) => (typeof a === "object" ? `(${psq('"')} + $env:${a.secret} + ${psq('"')})` : psq(winArg(a)));
  return [
    "$q = @'",
    sql,
    "'@",
    "$f = New-TemporaryFile; $o = New-TemporaryFile; $e = New-TemporaryFile",
    "[IO.File]::WriteAllText($f.FullName, $q)",
    ...env.map(([k, v]) => `$env:${k} = ${typeof v === "object" ? `$env:${v.secret}` : psq(v)}`),
    `$a = ${args.map(psArg).join(" + ' ' + ")}`,
    `$p = Start-Process -FilePath ${psq(program)} -ArgumentList $a -RedirectStandardInput $f.FullName -RedirectStandardOutput $o.FullName -RedirectStandardError $e.FullName -NoNewWindow -Wait -PassThru`,
    `Get-Content $o.FullName -TotalCount ${maxRows + 1} -Encoding UTF8`,
    "Get-Content $e.FullName -Encoding UTF8",
    `"\`n${ROWS_MARK} " + (Get-Content $o.FullName | Measure-Object -Line).Lines`,
    "Remove-Item $f.FullName, $o.FullName, $e.FullName",
    "exit $p.ExitCode",
  ].join("\n");
}

/** Reads `buildCommand`'s output into `{ columns, rows, total, truncated }`. */
export function parseResult(output, kind, maxRows) {
  const text = String(output).replace(/\r\n/g, "\n");
  const mark = text.lastIndexOf(ROWS_MARK);
  const lines = mark < 0 ? null : Number(text.slice(mark + ROWS_MARK.length).trim().split(/\s/)[0]);
  const body = (mark < 0 ? text : text.slice(0, mark)).replace(/\n+$/, "");
  if (!body.trim()) return { columns: [], rows: [], total: 0, truncated: false };
  const all = kind === "mysql" ? parseTsv(body) : parseCsv(body);
  const [columns = [], ...rows] = all;
  // All of the output came back: count the parsed rows (a value can span lines). Else estimate.
  const complete = lines === null || Number.isNaN(lines) || lines <= maxRows + 1;
  const total = complete ? rows.length : Math.max(rows.length, lines - 1);
  return { columns, rows: rows.slice(0, maxRows), total, truncated: total > maxRows };
}

/** The CLI's error message without the row marker. */
export function errorText(output) {
  const text = String(output);
  const mark = text.lastIndexOf(ROWS_MARK);
  return (mark < 0 ? text : text.slice(0, mark)).trim() || "The query failed without a message.";
}

const sqlString = (s) => `'${String(s).replace(/'/g, "''")}'`;

/** A query listing tables and their columns (of one table, if given). */
export function schemaSql(kind, table) {
  if (kind === "postgres") {
    return `SELECT table_schema, table_name, column_name, data_type, is_nullable FROM information_schema.columns WHERE table_schema NOT IN ('pg_catalog', 'information_schema')${table ? ` AND table_name = ${sqlString(table)}` : ""} ORDER BY table_schema, table_name, ordinal_position`;
  }
  if (kind === "mysql") {
    return `SELECT table_schema, table_name, column_name, data_type, is_nullable FROM information_schema.columns WHERE table_schema = DATABASE()${table ? ` AND table_name = ${sqlString(table)}` : ""} ORDER BY table_name, ordinal_position`;
  }
  return `SELECT 'main' AS table_schema, m.name AS table_name, p.name AS column_name, p.type AS data_type, CASE p."notnull" WHEN 1 THEN 'NO' ELSE 'YES' END AS is_nullable FROM sqlite_master m JOIN pragma_table_info(m.name) p WHERE m.type IN ('table', 'view') AND m.name NOT LIKE 'sqlite_%'${table ? ` AND m.name = ${sqlString(table)}` : ""} ORDER BY m.name, p.cid`;
}

/** Schema rows grouped as `[{ schema, name, columns: [{ name, type, nullable }] }]`. */
export function groupSchema(result) {
  const idx = (name) => result.columns.indexOf(name);
  const [s, t, c, d, n] = ["table_schema", "table_name", "column_name", "data_type", "is_nullable"].map(idx);
  const tables = new Map();
  for (const row of result.rows) {
    const id = `${row[s]}.${row[t]}`;
    if (!tables.has(id)) tables.set(id, { schema: row[s], name: row[t], columns: [] });
    tables.get(id).columns.push({ name: row[c], type: row[d], nullable: row[n] === "YES" });
  }
  return [...tables.values()];
}

/** A result as a plain-text table for agents, cut at `maxChars`. */
export function formatResult(result, maxChars = AGENT_CHARS) {
  if (result.columns.length === 0) return "(no rows)";
  const cell = (v) => {
    const s = String(v).replace(/\s+/g, " ");
    return s.length > 200 ? `${s.slice(0, 200)}…` : s;
  };
  const lines = [result.columns.map(cell).join(" | ")];
  let size = lines[0].length;
  let shown = 0;
  for (const row of result.rows) {
    const line = row.map(cell).join(" | ");
    if (size + line.length + 1 > maxChars) break;
    lines.push(line);
    size += line.length + 1;
    shown++;
  }
  const more = result.total - shown;
  if (more > 0) lines.push(`… ${more} more row${more === 1 ? "" : "s"} not shown${result.truncated ? " (add a WHERE or LIMIT, or raise max_rows)" : ""}`);
  return lines.join("\n");
}

const keyName = (text) => `DB_${String(text).toUpperCase().replace(/[^A-Z0-9_]+/g, "_").replace(/^_+|_+$/g, "")}`;
const needsApproval = (conn) => conn.meta.approve !== false;

/** "app@db.example.com:5432/app" — where a connection points, without its password. */
function target(meta) {
  if (meta.kind === "sqlite") return meta.path;
  return `${meta.user ? `${meta.user}@` : ""}${meta.host || "localhost"}${meta.port ? `:${meta.port}` : ""}${meta.database ? `/${meta.database}` : ""}`;
}

function describe(conn) {
  const meta = conn.meta;
  const parts = [`${conn.key} (${meta.name || conn.key}): ${KINDS[meta.kind]?.name ?? meta.kind} ${target(meta)}`];
  if (meta.description) parts.push(String(meta.description));
  parts.push(needsApproval(conn) ? "the user approves each query" : "queries run without asking");
  return parts.join(" — ");
}

const plural = (n, word) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

function ago(ms) {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86_400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86_400)} d ago`;
}

// Plugin ----------------------------------------------------------------------------------------

function createStore(api) {
  let history = [];
  try {
    history = JSON.parse(localStorage.getItem(HISTORY) ?? "[]") ?? [];
  } catch {
    history = [];
  }
  const listeners = new Set();
  let home = null;

  async function homeDir() {
    if (home === null) {
      const r = await api.vault.exec({ command: IS_WIN ? "echo %USERPROFILE%" : 'echo "$HOME"', cwd: "", keys: [], timeoutSec: 15 });
      home = r.output.trim();
    }
    return home;
  }
  /** vault.exec runs in "" for home but doesn't expand "~". */
  async function absolute(dir) {
    if (!dir || dir === "~") return "";
    if (!dir.startsWith("~/") && !dir.startsWith("~\\")) return dir;
    return (await homeDir()) + dir.slice(1);
  }

  async function connections() {
    return (await api.vault.list()).filter((s) => KINDS[s.meta?.kind]).sort((a, b) => String(a.meta.name || a.key).localeCompare(String(b.meta.name || b.key)));
  }

  /** A connection by key or name, for agents; throws listing the known ones. */
  async function find(name) {
    const all = await connections();
    const wanted = String(name ?? "").trim().toLowerCase();
    const hit = all.find((c) => c.key.toLowerCase() === wanted || String(c.meta.name ?? "").toLowerCase() === wanted || keyName(wanted).toLowerCase() === c.key.toLowerCase());
    if (hit) return hit;
    const known = all.map((c) => c.key).join(", ") || "none yet";
    throw new Error(`Unknown connection “${name}”. Connections: ${known}. The user can add one on winfer's DB Explorer page.`);
  }

  /** Runs one read statement; `{ ok, result?, error?, ms }`. */
  async function run(conn, sql, { maxRows = 200, timeoutSec = 60 } = {}) {
    const started = Date.now();
    const problem = checkReadOnly(sql, conn.meta.kind);
    if (problem) return { ok: false, error: problem, ms: 0 };
    try {
      const keys = conn.meta.kind === "postgres" || (conn.meta.kind === "mysql" && conn.meta.hasPassword) ? [conn.key] : [];
      const command = buildCommand(conn, sql, { maxRows, timeoutSec });
      const r = await api.vault.exec({ command: IS_WIN ? powershell(command) : command, cwd: await absolute(conn.meta.base ?? ""), keys, timeoutSec });
      const ms = Date.now() - started;
      if (r.timedOut) return { ok: false, error: `Stopped after ${timeoutSec} s.`, ms };
      if (r.code !== 0) {
        const text = errorText(r.output);
        const missing = new RegExp(`(${KINDS[conn.meta.kind].cli}: (command )?not found|not recognized as|cannot find the file)`, "i").test(text);
        return { ok: false, error: missing ? `${KINDS[conn.meta.kind].cli} isn't installed or isn't on the PATH. Install the ${KINDS[conn.meta.kind].name} command-line client and try again.\n\n${text}` : text, ms };
      }
      return { ok: true, result: parseResult(r.output, conn.meta.kind, maxRows), ms };
    } catch (err) {
      return { ok: false, error: String(err?.message ?? err), ms: Date.now() - started };
    }
  }

  function record(entry) {
    history = [{ id: `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`, at: Date.now(), ...entry }, ...history].slice(0, MAX_HISTORY);
    try {
      localStorage.setItem(HISTORY, JSON.stringify(history));
    } catch {
      // Storage full or off: the history just isn't kept.
    }
    listeners.forEach((fn) => fn());
  }

  function clearHistory(key) {
    history = key ? history.filter((h) => h.conn !== key) : [];
    localStorage.setItem(HISTORY, JSON.stringify(history));
    listeners.forEach((fn) => fn());
  }

  return {
    connections,
    find,
    run,
    record,
    clearHistory,
    history: () => history,
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    changed: () => listeners.forEach((fn) => fn()),
  };
}

/** Wraps a PowerShell script so it reaches PowerShell intact through `cmd /C`. */
function powershell(script) {
  const text = `$ProgressPreference = 'SilentlyContinue'; [Console]::OutputEncoding = [Text.Encoding]::UTF8; $OutputEncoding = [Text.Encoding]::UTF8\n${script}`;
  let bytes = "";
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    bytes += String.fromCharCode(c & 0xff, c >> 8);
  }
  return `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${btoa(bytes)}`;
}

export function activate(api) {
  const store = createStore(api);
  const clampRows = (v, def) => Math.min(5000, Math.max(1, Math.round(Number(v)) || def));

  api.registerAgentTool({
    name: "db_list",
    description: "List the databases the user connected in winfer's DB Explorer (Postgres, MySQL, SQLite), by name, with kind, host and database. Credentials are never shown. Query them read-only with db_schema and db_query.",
    inputSchema: { type: "object", properties: {} },
    async run() {
      const all = await store.connections();
      if (all.length === 0) return "No databases connected yet. Ask the user to add one on winfer's DB Explorer page.";
      return all.map(describe).join("\n");
    },
  });

  api.registerAgentTool({
    name: "db_schema",
    description: "List the tables of a connected database with their columns and types (or the columns of one table). Use it before writing a query.",
    inputSchema: {
      type: "object",
      properties: {
        connection: { type: "string", description: "Connection name or key from db_list, e.g. DB_MAIN" },
        table: { type: "string", description: "Only this table (optional)" },
      },
      required: ["connection"],
    },
    async run(args, caller) {
      const conn = await store.find(args.connection);
      const table = args.table ? String(args.table) : "";
      const res = await store.run(conn, schemaSql(conn.meta.kind, table), { maxRows: 5000 });
      if (!res.ok) throw new Error(res.error);
      store.record({ conn: conn.key, sql: `-- schema${table ? ` of ${table}` : ""}`, by: caller.label, rows: res.result.total, ms: res.ms, schema: true });
      const tables = groupSchema(res.result);
      if (tables.length === 0) return table ? `No table named ${table}.` : "The database has no tables.";
      const multiSchema = new Set(tables.map((t) => t.schema)).size > 1;
      const text = tables
        .map((t) => `${multiSchema ? `${t.schema}.` : ""}${t.name}(${t.columns.map((c) => `${c.name} ${c.type}${c.nullable ? "" : " not null"}`).join(", ")})`)
        .join("\n");
      return text.length > AGENT_CHARS ? `${text.slice(0, AGENT_CHARS)}\n… cut; ask for one table with "table".` : text;
    },
  });

  api.registerAgentTool({
    name: "db_query",
    description:
      "Run one read-only SQL statement (SELECT, WITH, EXPLAIN, SHOW…) on a connected database and get the rows back as a table. Writes are refused by the database session. Add a LIMIT for big tables. Use db_schema first to learn the tables.",
    inputSchema: {
      type: "object",
      properties: {
        connection: { type: "string", description: "Connection name or key from db_list" },
        sql: { type: "string", description: "One read-only SQL statement" },
        max_rows: { type: "integer", description: "Return at most this many rows (default 100, max 5000)" },
      },
      required: ["connection", "sql"],
    },
    async run(args, caller) {
      const conn = await store.find(args.connection);
      const sql = String(args.sql ?? "").trim().replace(/;\s*$/, "");
      const problem = checkReadOnly(sql, conn.meta.kind);
      if (problem) throw new Error(problem);
      if (needsApproval(conn)) {
        const ok = await api.confirm(`${caller.label} in “${caller.workspaceName}” wants to query ${conn.meta.name || conn.key} (read-only):\n\n${sql}`, { title: "Allow database query?", okLabel: "Run query", kind: "info" });
        if (!ok) {
          store.record({ conn: conn.key, sql, by: caller.label, error: "Declined by you", ms: 0 });
          throw new Error("The user declined this query.");
        }
      }
      const res = await store.run(conn, sql, { maxRows: clampRows(args.max_rows, 100) });
      store.record({ conn: conn.key, sql, by: caller.label, rows: res.ok ? res.result.total : undefined, error: res.ok ? undefined : res.error, ms: res.ms });
      if (!res.ok) throw new Error(res.error);
      const { result } = res;
      return `${plural(result.total, "row")} in ${res.ms} ms\n\n${formatResult(result)}`;
    },
  });

  api.registerPage({ id: "db", title: "Databases", icon: "database", mount: (el) => mountPage(el, api, store) });

  api.registerTool({
    id: "open",
    title: "DB Explorer",
    description: "Open the database explorer to browse tables, run read-only SQL queries and see the query history",
    palette: [{ title: "DB Explorer: open databases" }],
    run: () => api.openPage("db"),
  });
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
  db: "M12 3c4.4 0 8 1.3 8 3s-3.6 3-8 3-8-1.3-8-3 3.6-3 8-3zM4 6v6c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12v6c0 1.7 3.6 3 8 3s8-1.3 8-3v-6",
  play: "M6 4l14 8-14 8z",
  plus: "M12 5v14M5 12h14",
  pencil: "M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z",
  trash: "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6",
  table: "M3 4h18v16H3zM3 10h18M9 4v16",
  chevron: "M9 6l6 6-6 6",
  refresh: "M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5",
  copy: "M9 9h11v11H9zM5 15H4V4h11v1",
  check: "M20 6 9 17l-5-5",
  shield: "M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10zM9 12l2 2 4-4",
  bolt: "M13 2 3 14h9l-1 8 10-12h-9z",
  alert: "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
  lock: "M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4",
  clock: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 6v6l4 2",
  bot: "M12 8V4H8M4 8h16v12H4zM2 14h2M20 14h2M9 13v2M15 13v2",
  user: "M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2M12 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
  search: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3",
};

function icon(name) {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("class", "db-icon");
  for (const [k, v] of Object.entries({ fill: "none", stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round", "stroke-linejoin": "round" })) el.setAttribute(k, v);
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", ICONS[name]);
  el.append(path);
  return el;
}

const kindTile = (kind) => h("div", { class: `db-kind db-${kind}` }, KINDS[kind]?.short.slice(0, 2) ?? "DB");

const CSS = `
.db-page { max-width: 1280px; gap: 16px; container-type: inline-size; }
.db-icon { width: 14px; height: 14px; flex-shrink: 0; }
.db-hero { display: flex; align-items: center; gap: 14px; }
.db-hero-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.db-logo { width: 40px; height: 40px; border-radius: 11px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; color: white; background: linear-gradient(160deg, #5aa7f0, #2f6fd1); box-shadow: inset 0 1px 0 rgb(255 255 255 / .25), 0 4px 14px rgb(47 111 209 / .25); }
.db-logo .db-icon { width: 20px; height: 20px; }
.db-pill { display: inline-flex; align-items: center; gap: 6px; height: 24px; padding: 0 10px; border-radius: 999px; font-size: 11px; font-weight: 500; white-space: nowrap; color: var(--color-fg-muted); background: var(--color-panel-raised); border: 1px solid var(--color-border); }
.db-pill .db-icon { width: 12px; height: 12px; color: var(--color-success); }
@container (max-width: 700px) { .db-pill { display: none; } }
.db-label { display: flex; align-items: center; gap: 8px; font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: .05em; color: var(--color-fg-subtle); padding: 0 2px; }
.db-label > .db-grow { flex: 1; }
.db-grid { display: grid; grid-template-columns: 280px minmax(0, 1fr); gap: 16px; align-items: start; }
@container (max-width: 820px) { .db-grid { grid-template-columns: minmax(0, 1fr); } }
.db-side, .db-main { display: flex; flex-direction: column; gap: 10px; min-width: 0; }

.db-kind { --db-tone: var(--color-fg-subtle); width: 30px; height: 30px; border-radius: 8px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; font-size: 11px; font-weight: 700; letter-spacing: -.02em; color: var(--db-tone); background: color-mix(in srgb, var(--db-tone) 13%, transparent); border: 1px solid color-mix(in srgb, var(--db-tone) 22%, transparent); }
.db-kind.db-postgres { --db-tone: #4f8fdc; }
.db-kind.db-mysql { --db-tone: #e3973a; }
.db-kind.db-sqlite { --db-tone: #2bb39e; }

.db-conn { display: flex; align-items: center; gap: 10px; padding: 9px 8px 9px 10px; cursor: pointer; transition: background .12s; }
.db-conn + .db-conn { border-top: 1px solid var(--color-border); }
.db-conn:hover { background: color-mix(in srgb, var(--color-hover) 55%, transparent); }
.db-conn.db-on { background: var(--color-accent-soft); }
.db-conn-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.db-conn-name { display: flex; align-items: center; gap: 6px; font-size: 12px; font-weight: 600; color: var(--color-fg); }
.db-conn-name > span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.db-conn-target { font-family: var(--font-mono); font-size: 11px; color: var(--color-fg-subtle); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.db-conn-actions { display: flex; gap: 0; opacity: 0; transition: opacity .12s; }
.db-conn:hover .db-conn-actions, .db-conn.db-on .db-conn-actions { opacity: 1; }
.db-tag { display: inline-flex; align-items: center; gap: 3px; height: 16px; padding: 0 5px; border-radius: 4px; font-size: 10px; font-weight: 500; white-space: nowrap; color: var(--color-fg-subtle); background: var(--color-active); }
.db-tag .db-icon { width: 10px; height: 10px; }
.db-tag.db-ask { color: var(--color-accent); background: var(--color-accent-soft); }
.db-tag.db-auto { color: var(--color-warning); background: color-mix(in srgb, var(--color-warning) 12%, transparent); }
.db-icon-btn { display: inline-flex; align-items: center; justify-content: center; width: 26px; height: 26px; flex-shrink: 0; border-radius: 6px; border: 0; background: transparent; color: var(--color-fg-subtle); }
.db-icon-btn:hover { background: var(--color-active); color: var(--color-fg); }
.db-icon-btn.db-bad:hover { color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 12%, transparent); }
.db-icon-btn.db-done { color: var(--color-success); }
.db-icon-btn .db-icon { width: 13px; height: 13px; }

.db-schema { max-height: 420px; overflow: auto; padding: 4px 0; }
.db-filter { display: flex; align-items: center; gap: 6px; margin: 4px 8px 6px; height: 26px; padding: 0 8px; border-radius: 6px; border: 1px solid var(--color-border); background: var(--color-panel); }
.db-filter .db-icon { width: 12px; height: 12px; color: var(--color-fg-subtle); }
.db-filter input { flex: 1; min-width: 0; border: 0; outline: none; background: transparent; color: var(--color-fg); font: inherit; font-size: 12px; user-select: text; }
.db-table { display: flex; align-items: center; gap: 6px; width: 100%; cursor: pointer; padding: 4px 10px; border: 0; background: transparent; font: inherit; font-size: 12px; color: var(--color-fg); text-align: left; }
.db-table:hover { background: var(--color-hover); }
.db-table .db-icon { width: 12px; height: 12px; color: var(--color-fg-subtle); transition: transform .12s; }
.db-table.db-open > .db-icon:first-child { transform: rotate(90deg); }
.db-table > span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); }
.db-table small { color: var(--color-fg-subtle); font-size: 10px; }
.db-col { display: flex; gap: 8px; padding: 2px 10px 2px 38px; font-family: var(--font-mono); font-size: 11px; color: var(--color-fg-muted); cursor: pointer; }
.db-col:hover { background: var(--color-hover); color: var(--color-fg); }
.db-col span:first-child { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.db-col span:last-child { color: var(--color-fg-subtle); white-space: nowrap; }
.db-note { padding: 14px 12px; font-size: 12px; color: var(--color-fg-subtle); text-align: center; }
.db-note.db-err { color: var(--color-danger); text-align: left; white-space: pre-wrap; font-family: var(--font-mono); font-size: 11px; user-select: text; }

.db-editor { display: flex; flex-direction: column; }
.db-editor textarea { min-height: 120px; resize: vertical; padding: 12px 14px; border: 0; outline: none; background: transparent; color: var(--color-fg); font-family: var(--font-mono); font-size: 12.5px; line-height: 1.6; tab-size: 2; user-select: text; }
.db-editor textarea::placeholder { color: var(--color-fg-subtle); }
.db-bar { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; padding: 8px 10px; border-top: 1px solid var(--color-border); background: var(--color-panel); }
.db-bar .db-grow { flex: 1; }
.db-kbd { font-family: var(--font-mono); font-size: 10px; padding: 1px 5px; border-radius: 4px; background: rgb(255 255 255 / .18); }
.db-status { font-size: 11px; color: var(--color-fg-subtle); font-variant-numeric: tabular-nums; }
.db-select { height: 26px; padding: 0 6px; border-radius: 6px; border: 1px solid var(--color-border); background: var(--color-panel); color: var(--color-fg-muted); font: inherit; font-size: 11px; }

.db-results { overflow: auto; max-height: 520px; }
.db-grid-table { border-collapse: separate; border-spacing: 0; font-size: 12px; min-width: 100%; }
.db-grid-table th { position: sticky; top: 0; z-index: 1; text-align: left; font-weight: 600; padding: 7px 12px; background: var(--color-panel-raised); border-bottom: 1px solid var(--color-border); color: var(--color-fg); white-space: nowrap; font-family: var(--font-mono); font-size: 11.5px; }
.db-grid-table td { padding: 5px 12px; border-bottom: 1px solid var(--color-border); font-family: var(--font-mono); font-size: 11.5px; color: var(--color-fg-muted); max-width: 420px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; user-select: text; }
.db-grid-table tr:hover td { background: color-mix(in srgb, var(--color-hover) 55%, transparent); }
.db-grid-table td.db-null { color: var(--color-fg-subtle); font-style: italic; }
.db-grid-table td.db-num { text-align: right; font-variant-numeric: tabular-nums; }
.db-grid-table th.db-rn, .db-grid-table td.db-rn { color: var(--color-fg-subtle); text-align: right; width: 1%; padding-right: 8px; }
.db-result-head { display: flex; align-items: center; gap: 8px; padding: 8px 10px 8px 14px; border-bottom: 1px solid var(--color-border); font-size: 12px; color: var(--color-fg-muted); }
.db-result-head .db-grow { flex: 1; }
.db-error { display: flex; gap: 8px; align-items: flex-start; padding: 12px 14px; font-size: 12px; line-height: 1.5; color: var(--color-danger); white-space: pre-wrap; font-family: var(--font-mono); user-select: text; }
.db-error .db-icon { margin-top: 2px; }

.db-history-item { display: flex; align-items: flex-start; gap: 10px; padding: 8px 10px 8px 12px; cursor: pointer; }
.db-history-item + .db-history-item { border-top: 1px solid var(--color-border); }
.db-history-item:hover { background: color-mix(in srgb, var(--color-hover) 55%, transparent); }
.db-history-item > .db-icon { margin-top: 2px; color: var(--color-fg-subtle); }
.db-history-item.db-agent > .db-icon { color: var(--color-accent); }
.db-history-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.db-history-sql { font-family: var(--font-mono); font-size: 11.5px; color: var(--color-fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.db-history-meta { font-size: 11px; color: var(--color-fg-subtle); }
.db-history-meta .db-bad { color: var(--color-danger); }

.db-empty { display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 40px 20px 32px; text-align: center; }
.db-empty .db-logo { margin-bottom: 8px; }
.db-empty-title { font-size: 13px; font-weight: 600; color: var(--color-fg); }
.db-starters { display: flex; gap: 8px; flex-wrap: wrap; justify-content: center; margin-top: 14px; }
.db-starter { display: flex; align-items: center; gap: 10px; padding: 8px 12px 8px 8px; border-radius: 9px; border: 1px solid var(--color-border); background: var(--color-panel); font: inherit; font-size: 12px; font-weight: 500; color: var(--color-fg); }
.db-starter:hover { border-color: var(--color-border-strong); background: var(--color-hover); }
.db-starter .db-kind { width: 26px; height: 26px; border-radius: 7px; font-size: 10px; }

.db-dialog { width: min(520px, 92vw); padding: 0; gap: 0; }
.db-dialog-head { display: flex; align-items: center; gap: 12px; padding: 18px 20px 14px; }
.db-dialog-title { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.db-dialog-body { display: flex; flex-direction: column; gap: 14px; padding: 4px 20px 18px; }
.db-dialog-foot { display: flex; align-items: center; gap: 8px; padding: 12px 20px; border-top: 1px solid var(--color-border); background: var(--color-panel); }
.db-dialog-foot .db-grow { flex: 1; font-size: 11px; color: var(--color-fg-subtle); }
.db-field { display: flex; flex-direction: column; gap: 6px; font-size: 12px; font-weight: 500; color: var(--color-fg-muted); }
.db-field small { font-size: 11px; font-weight: 400; color: var(--color-fg-subtle); }
.db-input { display: flex; align-items: center; gap: 6px; height: 32px; padding: 0 10px; border-radius: 8px; border: 1px solid var(--color-border-strong); background: var(--color-panel); transition: border-color .15s, box-shadow .15s; }
.db-input:focus-within { border-color: var(--color-accent); box-shadow: 0 0 0 3px var(--color-accent-soft); }
.db-input input { flex: 1; min-width: 0; height: 100%; border: 0; outline: none; background: transparent; color: var(--color-fg); font: inherit; font-size: 12px; user-select: text; }
.db-input input.db-mono { font-family: var(--font-mono); }
.db-input input::placeholder { color: var(--color-fg-subtle); }
.db-kinds { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 6px; }
.db-kind-opt { display: flex; align-items: center; gap: 8px; padding: 8px 10px; border-radius: 9px; border: 1px solid var(--color-border); background: var(--color-panel); font: inherit; font-size: 12px; font-weight: 500; color: var(--color-fg-muted); }
.db-kind-opt:hover { border-color: var(--color-border-strong); color: var(--color-fg); }
.db-kind-opt .db-kind { width: 24px; height: 24px; border-radius: 6px; font-size: 10px; }
.db-kind-opt.db-on { border-color: var(--color-accent); background: color-mix(in srgb, var(--color-accent) 8%, var(--color-panel)); color: var(--color-fg); }
.db-approve { display: flex; align-items: center; gap: 12px; padding: 10px 12px; border-radius: 9px; border: 1px solid var(--color-border); background: var(--color-panel); }
.db-approve > .db-icon { width: 16px; height: 16px; color: var(--color-accent); }
.db-approve.db-off > .db-icon { color: var(--color-warning); }
.db-approve > div { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.db-approve b { font-size: 12px; font-weight: 500; color: var(--color-fg); }
.db-approve span { font-size: 11px; line-height: 1.45; color: var(--color-fg-subtle); }
.db-alert { display: flex; gap: 8px; align-items: flex-start; padding: 9px 12px; border-radius: 8px; font-size: 12px; line-height: 1.5; color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 9%, transparent); border: 1px solid color-mix(in srgb, var(--color-danger) 25%, transparent); white-space: pre-wrap; user-select: text; }
.db-alert.db-ok { color: var(--color-success); background: color-mix(in srgb, var(--color-success) 9%, transparent); border-color: color-mix(in srgb, var(--color-success) 25%, transparent); }
.db-alert .db-icon { margin-top: 2px; }
`;

function mountPage(root, api, store) {
  let conns = [];
  let selected = null;
  let loadError = "";
  /** Schema per connection key: { state: "loading" | "ok" | "error", tables, error }. */
  const schemas = new Map();
  const openTables = new Set();
  let tableFilter = "";
  let maxRows = 500;
  let running = false;
  /** The last run: { conn, result?, error?, ms }. */
  let last = null;
  let copied = false;
  /** Editor text per connection, kept while the page is open. */
  const drafts = new Map();

  const style = h("style", {}, CSS);
  const page = h("div", { class: "wf-page db-page" });
  root.append(style, page);

  const current = () => conns.find((c) => c.key === selected) ?? null;

  async function refresh() {
    try {
      conns = await store.connections();
      loadError = "";
    } catch (err) {
      loadError = String(err?.message ?? err);
    }
    if (!current()) selected = conns[0]?.key ?? null;
    render();
    if (current() && !schemas.has(selected)) void loadSchema(current());
  }

  async function loadSchema(conn) {
    schemas.set(conn.key, { state: "loading" });
    renderSide();
    const res = await store.run(conn, schemaSql(conn.meta.kind), { maxRows: 20_000 });
    schemas.set(conn.key, res.ok ? { state: "ok", tables: groupSchema(res.result) } : { state: "error", error: res.error });
    renderSide();
  }

  function select(key) {
    if (selected === key) return;
    if (selected) drafts.set(selected, editor.value);
    selected = key;
    editor.value = drafts.get(key) ?? "";
    last = null;
    tableFilter = "";
    filterInput.value = "";
    render();
    const conn = current();
    if (conn && !schemas.has(key)) void loadSchema(conn);
  }

  async function runQuery() {
    const conn = current();
    if (!conn || running) return;
    const text = editor.value;
    const chosen = editor.selectionStart !== editor.selectionEnd ? text.slice(editor.selectionStart, editor.selectionEnd) : statementAt(text, conn.meta.kind, editor.selectionStart);
    const sql = chosen.trim().replace(/;\s*$/, "");
    if (!sql) return;
    running = true;
    renderMain();
    const res = await store.run(conn, sql, { maxRows });
    running = false;
    last = { conn: conn.key, ...res };
    store.record({ conn: conn.key, sql, by: "you", rows: res.ok ? res.result.total : undefined, error: res.ok ? undefined : res.error, ms: res.ms });
    renderMain();
  }

  function insert(text) {
    const start = editor.selectionStart;
    editor.setRangeText(text, start, editor.selectionEnd, "end");
    editor.focus();
  }

  const quoteName = (kind, name) => (/^[a-z_][a-z0-9_]*$/.test(name) ? name : kind === "mysql" ? `\`${name.replace(/`/g, "``")}\`` : `"${name.replace(/"/g, '""')}"`);

  function previewTable(conn, table) {
    const name = (table.schema && table.schema !== "public" && table.schema !== "main" && conn.meta.kind === "postgres" ? `${quoteName("postgres", table.schema)}.` : "") + quoteName(conn.meta.kind, table.name);
    editor.value = `SELECT * FROM ${name} LIMIT 100`;
    editor.setSelectionRange(0, 0);
    void runQuery();
  }

  const btn = (name, title, onclick, extra = "") =>
    h("button", { class: `db-icon-btn ${extra}`, title, "aria-label": title, onclick: (e) => (e.stopPropagation(), onclick(e)) }, icon(name));

  // Static parts; render() fills them.
  const pill = h("span", { class: "db-pill" });
  const problem = h("div", { class: "db-alert" });
  const body = h("div");
  const connList = h("div", { class: "wf-card" });
  const filterInput = h("input", {
    type: "search",
    placeholder: "Filter tables",
    spellcheck: "false",
    oninput: (e) => {
      tableFilter = e.target.value.toLowerCase();
      renderSide();
    },
  });
  const schemaLabel = h("div", { class: "db-label" });
  const schemaList = h("div", { class: "db-schema" });
  const schemaCard = h("div", { class: "wf-card" }, h("label", { class: "db-filter" }, icon("search"), filterInput), schemaList);
  const editor = h("textarea", {
    spellcheck: "false",
    placeholder: "SELECT * FROM users LIMIT 100\n\n⌘↵ runs the statement at the cursor, or the selection.",
    onkeydown: (e) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        void runQuery();
      } else if (e.key === "Tab" && !e.shiftKey) {
        e.preventDefault();
        insert("  ");
      }
    },
  });
  const runButton = h("button", { class: "wf-button wf-primary", onclick: () => runQuery() });
  const limitSelect = h(
    "select",
    { class: "db-select", title: "Rows to show", onchange: (e) => (maxRows = Number(e.target.value)) },
    ROW_LIMITS.map((n) => h("option", { value: String(n), selected: n === maxRows }, `${n.toLocaleString()} rows`)),
  );
  const status = h("span", { class: "db-status" });
  const editorCard = h("div", { class: "wf-card db-editor" }, editor, h("div", { class: "db-bar" }, runButton, limitSelect, h("span", { class: "db-grow" }), status));
  const resultCard = h("div", { class: "wf-card" });
  const historyLabel = h("div", { class: "db-label" });
  const historyList = h("div", { class: "wf-card" });

  page.append(
    h(
      "div",
      { class: "db-hero" },
      h("div", { class: "db-logo" }, icon("db")),
      h("div", { class: "db-hero-text" }, h("h1", { class: "wf-h1" }, "Databases"), h("div", { class: "wf-muted" }, "Browse and query your databases read-only. Agents query them by name and never see the credentials.")),
      pill,
      h("button", { class: "wf-button wf-primary", onclick: () => openEditor(null) }, icon("plus"), "Add connection"),
    ),
    problem,
    body,
  );

  const grid = h(
    "div",
    { class: "db-grid" },
    h("div", { class: "db-side" }, h("div", { class: "db-label" }, "Connections"), connList, schemaLabel, schemaCard),
    h("div", { class: "db-main" }, editorCard, resultCard, historyLabel, historyList),
  );

  function empty() {
    return h(
      "div",
      { class: "wf-card" },
      h(
        "div",
        { class: "db-empty" },
        h("div", { class: "db-logo" }, icon("db")),
        h("div", { class: "db-empty-title" }, "No databases yet"),
        h("div", { class: "wf-muted", style: "max-width:420px" }, "Connect a database to browse its tables and run read-only queries. Credentials stay in your keychain; agents only get the connection's name."),
        h("div", { class: "db-starters" }, Object.keys(KINDS).map((kind) => h("button", { class: "db-starter", onclick: () => openEditor(null, kind) }, kindTile(kind), KINDS[kind].name))),
      ),
    );
  }

  function render() {
    problem.replaceChildren(icon("alert"), loadError);
    problem.style.display = loadError ? "" : "none";
    pill.replaceChildren(icon("lock"), "Read-only");
    if (conns.length === 0) {
      body.replaceChildren(empty());
      return;
    }
    if (body.firstChild !== grid) body.replaceChildren(grid);
    renderSide();
    renderMain();
  }

  function renderSide() {
    connList.replaceChildren(
      ...conns.map((c) =>
        h(
          "div",
          { class: `db-conn${c.key === selected ? " db-on" : ""}`, onclick: () => select(c.key), title: c.meta.description || c.key },
          kindTile(c.meta.kind),
          h(
            "div",
            { class: "db-conn-body" },
            h(
              "div",
              { class: "db-conn-name" },
              h("span", {}, c.meta.name || c.key),
              c.meta.approve !== false
                ? h("span", { class: "db-tag db-ask", title: "Agents need your OK for each query" }, icon("shield"), "Asks")
                : h("span", { class: "db-tag db-auto", title: "Agents query without asking you" }, icon("bolt"), "Auto"),
            ),
            h("div", { class: "db-conn-target" }, target(c.meta)),
          ),
          h(
            "div",
            { class: "db-conn-actions" },
            btn("pencil", "Edit", () => openEditor(c)),
            btn(
              "trash",
              "Remove",
              async () => {
                if (!(await api.confirm(`Remove ${c.meta.name || c.key}? Its credentials are deleted from your keychain. The database itself isn't touched.`, { title: "Remove connection", okLabel: "Remove" }))) return;
                try {
                  await api.vault.delete(c.key);
                  schemas.delete(c.key);
                  drafts.delete(c.key);
                } catch (err) {
                  api.toast(String(err?.message ?? err), "error");
                }
                await refresh();
              },
              "db-bad",
            ),
          ),
        ),
      ),
    );

    const conn = current();
    const schema = conn ? schemas.get(conn.key) : null;
    schemaLabel.replaceChildren(
      "Tables",
      schema?.state === "ok" ? h("span", { style: "font-weight:500;text-transform:none;letter-spacing:0" }, String(schema.tables.length)) : null,
      h("span", { class: "db-grow" }),
      conn ? btn("refresh", "Reload tables", () => loadSchema(conn)) : null,
    );
    if (!schema || schema.state === "loading") {
      schemaList.replaceChildren(h("div", { class: "db-note" }, "Loading tables…"));
      return;
    }
    if (schema.state === "error") {
      schemaList.replaceChildren(h("div", { class: "db-note db-err" }, schema.error));
      return;
    }
    const shown = schema.tables.filter((t) => !tableFilter || t.name.toLowerCase().includes(tableFilter) || t.columns.some((c) => c.name.toLowerCase().includes(tableFilter)));
    const multiSchema = new Set(schema.tables.map((t) => t.schema)).size > 1;
    if (shown.length === 0) {
      schemaList.replaceChildren(h("div", { class: "db-note" }, schema.tables.length ? "No table matches." : "No tables."));
      return;
    }
    schemaList.replaceChildren(
      ...shown.flatMap((t) => {
        const id = `${conn.key}:${t.schema}.${t.name}`;
        const open = openTables.has(id);
        const row = h(
          "div",
          {
            role: "button",
            tabindex: "0",
            class: `db-table${open ? " db-open" : ""}`,
            title: "Click to show columns, double-click to preview rows",
            onclick: () => {
              if (open) openTables.delete(id);
              else openTables.add(id);
              renderSide();
            },
            ondblclick: () => previewTable(conn, t),
          },
          icon("chevron"),
          h("span", {}, `${multiSchema ? `${t.schema}.` : ""}${t.name}`),
          h("small", {}, String(t.columns.length)),
          btn("play", "Preview rows", () => previewTable(conn, t)),
        );
        if (!open) return [row];
        return [
          row,
          ...t.columns.map((c) => h("div", { class: "db-col", title: "Insert the column name", onclick: () => insert(quoteName(conn.meta.kind, c.name)) }, h("span", {}, c.name), h("span", {}, `${c.type}${c.nullable ? "" : " ·"}`))),
        ];
      }),
    );
  }

  function resultView() {
    if (running) return h("div", { class: "db-note" }, "Running…");
    if (!last || last.conn !== selected) return h("div", { class: "db-note" }, "Results show up here.");
    if (!last.ok) return h("div", { class: "db-error" }, icon("alert"), last.error);
    const { columns, rows, total, truncated } = last.result;
    if (columns.length === 0) return h("div", { class: "db-note" }, "The statement returned no rows.");
    const numeric = columns.map((_, i) => rows.length > 0 && rows.every((r) => r[i] === "NULL" || r[i] === "" || /^-?\d+(\.\d+)?(e[+-]?\d+)?$/i.test(r[i] ?? "")));
    const copyCsv = async () => {
      const esc = (v) => (/[",\n]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v);
      await navigator.clipboard.writeText([columns, ...rows].map((r) => r.map((v) => esc(String(v ?? ""))).join(",")).join("\n"));
      copied = true;
      renderMain();
      setTimeout(() => ((copied = false), renderMain()), 1500);
    };
    return h(
      "div",
      {},
      h(
        "div",
        { class: "db-result-head" },
        h("span", {}, truncated ? `First ${plural(rows.length, "row")} of about ${total.toLocaleString()}` : plural(total, "row")),
        h("span", { class: "db-grow" }),
        copied ? btn("check", "Copied", () => {}, "db-done") : btn("copy", "Copy as CSV", copyCsv),
      ),
      h(
        "div",
        { class: "db-results" },
        h(
          "table",
          { class: "db-grid-table" },
          h("thead", {}, h("tr", {}, h("th", { class: "db-rn" }, "#"), columns.map((c) => h("th", {}, c)))),
          h(
            "tbody",
            {},
            rows.map((r, i) =>
              h(
                "tr",
                {},
                h("td", { class: "db-rn" }, String(i + 1)),
                columns.map((_, j) => {
                  const v = r[j] ?? "";
                  return h("td", { class: v === "NULL" ? "db-null" : numeric[j] ? "db-num" : "", title: v.length > 40 ? v : undefined }, v);
                }),
              ),
            ),
          ),
        ),
      ),
    );
  }

  function renderMain() {
    const conn = current();
    runButton.disabled = !conn || running;
    runButton.replaceChildren(icon("play"), running ? "Running…" : "Run", h("span", { class: "db-kbd" }, "⌘↵"));
    status.textContent = last && last.conn === selected && !running ? `${last.ok ? "Done" : "Failed"} in ${last.ms.toLocaleString()} ms` : conn ? `${KINDS[conn.meta.kind].short} · read-only` : "";
    resultCard.replaceChildren(resultView());

    const items = store.history().filter((it) => it.conn === selected).slice(0, 30);
    historyLabel.replaceChildren(
      "History",
      h("span", { class: "db-grow" }),
      items.length ? h("button", { class: "wf-button wf-ghost", style: "height:22px;font-size:11px", onclick: () => store.clearHistory(selected) }, "Clear") : null,
    );
    historyList.replaceChildren(
      ...(items.length === 0
        ? [h("div", { class: "db-note" }, "Queries you and your agents run show up here.")]
        : items.map((it) =>
            h(
              "div",
              {
                class: `db-history-item${it.by !== "you" ? " db-agent" : ""}`,
                title: "Load into the editor",
                onclick: () => {
                  if (it.schema) return;
                  editor.value = it.sql;
                  editor.focus();
                },
              },
              icon(it.by === "you" ? "user" : "bot"),
              h(
                "div",
                { class: "db-history-body" },
                h("div", { class: "db-history-sql" }, it.sql.replace(/\s+/g, " ")),
                h(
                  "div",
                  { class: "db-history-meta" },
                  `${it.by === "you" ? "You" : it.by} · ${ago(it.at)} · `,
                  it.error ? h("span", { class: "db-bad" }, it.error.split("\n")[0].slice(0, 120)) : `${plural(it.rows ?? 0, "row")} in ${(it.ms ?? 0).toLocaleString()} ms`,
                ),
              ),
            ),
          )),
    );
  }

  function openEditor(conn, initialKind = "postgres") {
    const editing = !!conn;
    const meta = conn?.meta ?? {};
    let kind = editing ? meta.kind : initialKind;
    const name = h("input", { value: meta.name ?? "", placeholder: "Main", spellcheck: "false" });
    const url = h("input", { class: "db-mono", type: "password", autocomplete: "off", spellcheck: "false" });
    const showUrl = h("button", { class: "db-icon-btn", type: "button", title: "Show", onclick: () => (url.type = url.type === "password" ? "text" : "password") }, icon("search"));
    const path = h("input", { class: "db-mono", value: meta.path ?? "", placeholder: KINDS.sqlite.placeholder, spellcheck: "false" });
    const description = h("input", { value: meta.description ?? "", placeholder: "e.g. Staging replica, safe to query" });
    const approveBox = h("input", { type: "checkbox", class: "wf-switch", checked: meta.approve !== false, onchange: () => approveRow.classList.toggle("db-off", !approveBox.checked) });
    const message = h("div", { class: "db-alert", style: "display:none" });
    const urlField = h("label", { class: "db-field" }, h("span", {}, "Connection URL ", h("small", {}, "· stored in your keychain")), h("div", { class: "db-input" }, url, showUrl));
    const pathField = h("label", { class: "db-field" }, h("span", {}, "Database file ", h("small", {}, "· absolute, or relative to the current project")), h("div", { class: "db-input" }, path));
    const kindButtons = h("div", { class: "db-kinds", role: "radiogroup" });
    const approveRow = h(
      "label",
      { class: `db-approve${meta.approve === false ? " db-off" : ""}` },
      icon("shield"),
      h("div", {}, h("b", {}, "Ask before agents query it"), h("span", {}, "You see each query first and can decline. Schema lookups don't ask.")),
      approveBox,
    );

    function setKind(next) {
      kind = next;
      urlField.style.display = kind === "sqlite" ? "none" : "";
      pathField.style.display = kind === "sqlite" ? "" : "none";
      url.placeholder = editing && meta.kind === kind ? "Leave empty to keep the current URL" : KINDS[kind].placeholder;
      kindButtons.replaceChildren(
        ...Object.keys(KINDS).map((id) =>
          h("button", { type: "button", role: "radio", disabled: editing, "aria-checked": String(id === kind), class: `db-kind-opt${id === kind ? " db-on" : ""}`, onclick: () => setKind(id) }, kindTile(id), KINDS[id].short),
        ),
      );
    }
    setKind(kind);

    const show = (text, ok = false) => {
      message.className = `db-alert${ok ? " db-ok" : ""}`;
      message.replaceChildren(icon(ok ? "check" : "alert"), text);
      message.style.display = "";
    };

    /** The connection as the vault stores it: `{ key, value, meta }` (value undefined keeps the old one). */
    function build() {
      const label = name.value.trim() || (kind === "sqlite" ? path.value.trim().split(/[\\/]/).pop()?.replace(/\.\w+$/, "") : parseDbUrl(url.value)?.database) || "";
      if (!label) throw new Error("Give the connection a name.");
      const key = editing ? conn.key : keyName(label);
      if (key === "DB_") throw new Error("Use letters or digits in the name.");
      if (!editing && conns.some((c) => c.key === key)) throw new Error(`A connection named ${label} already exists.`);
      const next = { kind, name: label, description: description.value.trim(), approve: approveBox.checked };
      if (kind === "sqlite") {
        if (!path.value.trim()) throw new Error("Enter the path of the database file.");
        const s = api.state();
        return { key, value: "-", meta: { ...next, path: path.value.trim(), base: s.workspaces.find((w) => w.id === s.activeWorkspaceId)?.rootDir ?? "" } };
      }
      const text = url.value.trim();
      if (!text) {
        if (!editing) throw new Error("Paste the connection URL.");
        return { key, value: undefined, meta: { ...meta, ...next } };
      }
      const parts = parseDbUrl(text);
      if (!parts) throw new Error(`That isn't a URL. It looks like ${KINDS[kind].placeholder}`);
      if (parts.kind !== kind) throw new Error(`That's not a ${KINDS[kind].name} URL; it should start with ${kind === "postgres" ? "postgres://" : "mysql://"}.`);
      const shown = { host: parts.host, port: parts.port, user: parts.user, database: parts.database };
      // psql takes the whole URL; mysql takes the parts as flags and only the password stays secret.
      if (kind === "postgres") return { key, value: text, meta: { ...next, ...shown } };
      return { key, value: parts.password || "-", meta: { ...next, ...shown, hasPassword: !!parts.password } };
    }

    async function save(e) {
      e.preventDefault();
      message.style.display = "none";
      try {
        const c = build();
        await api.vault.set(c.key, c.value, c.meta);
        schemas.delete(c.key);
        close();
        selected = c.key;
        await refresh();
      } catch (err) {
        show(String(err?.message ?? err));
      }
    }

    async function test() {
      message.style.display = "none";
      try {
        const c = build();
        // A changed URL only reaches the database through the keychain, so it's saved first.
        if (c.value !== undefined) await api.vault.set(c.key, c.value, c.meta);
        show("Connecting…", true);
        const res = await store.run({ key: c.key, meta: c.meta }, "SELECT 1", { maxRows: 1, timeoutSec: 20 });
        if (res.ok) show(`Connected in ${res.ms} ms.`, true);
        else show(res.error);
      } catch (err) {
        show(String(err?.message ?? err));
      }
    }

    const close = () => backdrop.remove();
    const backdrop = h(
      "div",
      { class: "wf-dialog-backdrop", onmousedown: (e) => e.target === backdrop && close() },
      h(
        "form",
        { class: "wf-dialog db-dialog", onsubmit: save, onkeydown: (e) => e.key === "Escape" && (e.stopPropagation(), close()) },
        h(
          "div",
          { class: "db-dialog-head" },
          h("div", { class: "db-logo" }, icon("db")),
          h("div", { class: "db-dialog-title" }, h("h2", { class: "wf-h2", style: "font-weight:600" }, editing ? `Edit ${meta.name || conn.key}` : "Add connection"), h("div", { class: "wf-subtle" }, "Queries always run read-only.")),
        ),
        h(
          "div",
          { class: "db-dialog-body" },
          h("div", { class: "db-field" }, "Database", kindButtons),
          h("label", { class: "db-field" }, h("span", {}, "Name ", h("small", {}, "· agents use it to pick the database")), h("div", { class: "db-input" }, name)),
          urlField,
          pathField,
          h("label", { class: "db-field" }, h("span", {}, "Description ", h("small", {}, "· optional, agents can see this")), h("div", { class: "db-input" }, description)),
          approveRow,
          message,
        ),
        h(
          "div",
          { class: "db-dialog-foot" },
          editing ? h("button", { class: "wf-button", type: "button", onclick: test }, "Test") : null,
          h("span", { class: "db-grow" }),
          h("button", { class: "wf-button wf-ghost", type: "button", onclick: close }, "Cancel"),
          h("button", { class: "wf-button wf-primary", type: "submit" }, editing ? "Save changes" : "Add connection"),
        ),
      ),
    );
    document.body.append(backdrop);
    (editing ? url : name).focus();
  }

  const unsubscribe = store.subscribe(() => conns.length && renderMain());
  render();
  void refresh();
  return () => {
    unsubscribe();
    document.querySelectorAll(".wf-dialog-backdrop").forEach((d) => d.remove());
    page.remove();
    style.remove();
  };
}
