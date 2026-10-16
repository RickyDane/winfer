// Schema Diagram: reads the structure of the user's Postgres, MySQL, SQLite and SQL Server databases and draws
// it as a class diagram: one box per table or view with its columns, keys and types, and a line per
// relation with its cardinality (crow's foot or UML multiplicities).
//
// Connections live in the plugin's vault like DB Explorer's: a Postgres URL or a MySQL password is a
// secret the CLI gets as an environment variable. Only catalog queries run (pg_catalog,
// information_schema, sqlite pragmas), in a read-only session, through `api.vault.exec`.
//
// The analysis is pure functions (introspection SQL, `buildModel`, `inferRelations`, `layout`,
// the exporters), tested in scripts/schema-diagram.test.mjs; `pnpm dev` with `?erd` opens the page
// on a fake shop database (src/dev/schemaDiagram.ts).

const IS_WIN = typeof navigator !== "undefined" && (/Win/i.test(navigator.platform ?? "") || /Windows/i.test(navigator.userAgent));
const LAYOUTS = "winfer.schema-diagram.layout.";
const PREFS = "winfer.schema-diagram.prefs";
const ROWS_MARK = "@@WINFER_ROWS";
/** Separates the columns of a key inside one cell of the catalog queries. */
const US = "\u001f";
/** What an agent sees, at most. */
const AGENT_CHARS = 16_000;

export const KINDS = {
  postgres: { name: "PostgreSQL", short: "Postgres", cli: "psql", placeholder: "postgres://user:password@localhost:5432/app" },
  mysql: { name: "MySQL / MariaDB", short: "MySQL", cli: "mysql", placeholder: "mysql://user:password@localhost:3306/app" },
  sqlite: { name: "SQLite", short: "SQLite", cli: "sqlite3", placeholder: "data/app.db" },
  mssql: { name: "SQL Server", short: "SQL Server", cli: "sqlcmd", placeholder: "sqlserver://user:password@localhost:1433/app" },
};

// Introspection SQL -------------------------------------------------------------------------------
//
// Three queries per dialect with the same output columns, so `buildModel` reads them all alike:
//   columns:     table_schema, table_name, table_type, column_name, data_type, is_nullable,
//                column_default, ordinal, row_estimate, table_comment, column_comment
//   constraints: table_schema, table_name, constraint_name, constraint_type (p, f, u), columns,
//                ref_schema, ref_table, ref_columns, on_delete, on_update
//   indexes:     table_schema, table_name, index_name, is_unique, is_primary, columns, method, is_partial
// Key columns are joined with US. Each query starts with a /* erd:<part> */ tag.

const PG_SKIP = "n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp%'";
const PG_RULE = (col) => `CASE ${col} WHEN 'c' THEN 'CASCADE' WHEN 'n' THEN 'SET NULL' WHEN 'd' THEN 'SET DEFAULT' WHEN 'r' THEN 'RESTRICT' WHEN 'a' THEN 'NO ACTION' ELSE '' END`;
const PG_KEY = (cols, rel) =>
  `COALESCE((SELECT string_agg(a.attname, '${US}' ORDER BY k.i) FROM unnest(${cols}) WITH ORDINALITY AS k(num, i) JOIN pg_attribute a ON a.attrelid = ${rel} AND a.attnum = k.num), '')`;

const SQL = {
  postgres: {
    columns: `/* erd:columns */
SELECT n.nspname AS table_schema, cl.relname AS table_name,
  CASE cl.relkind WHEN 'v' THEN 'view' WHEN 'm' THEN 'materialized view' WHEN 'f' THEN 'foreign table' ELSE 'table' END AS table_type,
  a.attname AS column_name, format_type(a.atttypid, a.atttypmod) AS data_type,
  CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END AS is_nullable,
  COALESCE(pg_get_expr(d.adbin, d.adrelid), CASE WHEN a.attidentity IN ('a', 'd') THEN 'identity' END, '') AS column_default,
  a.attnum AS ordinal,
  CASE WHEN cl.reltuples < 0 OR cl.relkind = 'v' THEN '' ELSE cl.reltuples::bigint::text END AS row_estimate,
  COALESCE(obj_description(cl.oid, 'pg_class'), '') AS table_comment,
  COALESCE(col_description(cl.oid, a.attnum), '') AS column_comment
FROM pg_attribute a
JOIN pg_class cl ON cl.oid = a.attrelid
JOIN pg_namespace n ON n.oid = cl.relnamespace
LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
WHERE a.attnum > 0 AND NOT a.attisdropped AND cl.relkind IN ('r', 'p', 'v', 'm', 'f') AND NOT cl.relispartition AND ${PG_SKIP}
ORDER BY 1, 2, a.attnum`,
    constraints: `/* erd:constraints */
SELECT n.nspname AS table_schema, cl.relname AS table_name, con.conname AS constraint_name, con.contype::text AS constraint_type,
  ${PG_KEY("con.conkey", "con.conrelid")} AS columns,
  COALESCE(fn.nspname, '') AS ref_schema, COALESCE(fcl.relname, '') AS ref_table,
  ${PG_KEY("con.confkey", "con.confrelid")} AS ref_columns,
  ${PG_RULE("con.confdeltype")} AS on_delete, ${PG_RULE("con.confupdtype")} AS on_update
FROM pg_constraint con
JOIN pg_class cl ON cl.oid = con.conrelid
JOIN pg_namespace n ON n.oid = cl.relnamespace
LEFT JOIN pg_class fcl ON fcl.oid = con.confrelid
LEFT JOIN pg_namespace fn ON fn.oid = fcl.relnamespace
WHERE con.contype IN ('p', 'f', 'u') AND con.conparentid = 0 AND NOT cl.relispartition AND ${PG_SKIP}
ORDER BY 1, 2, 3`,
    indexes: `/* erd:indexes */
SELECT n.nspname AS table_schema, t.relname AS table_name, i.relname AS index_name,
  CASE WHEN ix.indisunique THEN 'YES' ELSE 'NO' END AS is_unique,
  CASE WHEN ix.indisprimary THEN 'YES' ELSE 'NO' END AS is_primary,
  COALESCE((SELECT string_agg(COALESCE(a.attname, '(expression)'), '${US}' ORDER BY k.i) FROM unnest(ix.indkey::int2[]) WITH ORDINALITY AS k(num, i) LEFT JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.num AND k.num > 0), '') AS columns,
  am.amname AS method,
  CASE WHEN ix.indpred IS NULL THEN 'NO' ELSE 'YES' END AS is_partial
FROM pg_index ix
JOIN pg_class t ON t.oid = ix.indrelid
JOIN pg_class i ON i.oid = ix.indexrelid
JOIN pg_am am ON am.oid = i.relam
JOIN pg_namespace n ON n.oid = t.relnamespace
WHERE NOT t.relispartition AND ${PG_SKIP}
ORDER BY 1, 2, 3`,
  },
  mysql: {
    columns: `/* erd:columns */
SELECT c.table_schema AS table_schema, c.table_name AS table_name,
  CASE t.table_type WHEN 'VIEW' THEN 'view' ELSE 'table' END AS table_type,
  c.column_name AS column_name, c.column_type AS data_type, c.is_nullable AS is_nullable,
  COALESCE(c.column_default, CASE WHEN c.extra LIKE '%auto_increment%' THEN 'auto_increment' END, '') AS column_default,
  c.ordinal_position AS ordinal,
  COALESCE(t.table_rows, '') AS row_estimate,
  CASE WHEN t.table_type = 'VIEW' THEN '' ELSE COALESCE(t.table_comment, '') END AS table_comment,
  COALESCE(c.column_comment, '') AS column_comment
FROM information_schema.columns c
JOIN information_schema.tables t ON t.table_schema = c.table_schema AND t.table_name = c.table_name
WHERE c.table_schema = DATABASE()
ORDER BY c.table_name, c.ordinal_position`,
    constraints: `/* erd:constraints */
SELECT tc.table_schema AS table_schema, tc.table_name AS table_name, tc.constraint_name AS constraint_name,
  CASE tc.constraint_type WHEN 'PRIMARY KEY' THEN 'p' WHEN 'FOREIGN KEY' THEN 'f' ELSE 'u' END AS constraint_type,
  GROUP_CONCAT(k.column_name ORDER BY k.ordinal_position SEPARATOR '${US}') AS columns,
  COALESCE(MAX(k.referenced_table_schema), '') AS ref_schema, COALESCE(MAX(k.referenced_table_name), '') AS ref_table,
  COALESCE(GROUP_CONCAT(k.referenced_column_name ORDER BY k.ordinal_position SEPARATOR '${US}'), '') AS ref_columns,
  COALESCE(MAX(rc.delete_rule), '') AS on_delete, COALESCE(MAX(rc.update_rule), '') AS on_update
FROM information_schema.table_constraints tc
JOIN information_schema.key_column_usage k ON k.constraint_schema = tc.constraint_schema AND k.constraint_name = tc.constraint_name AND k.table_schema = tc.table_schema AND k.table_name = tc.table_name
LEFT JOIN information_schema.referential_constraints rc ON rc.constraint_schema = tc.constraint_schema AND rc.constraint_name = tc.constraint_name AND rc.table_name = tc.table_name
WHERE tc.table_schema = DATABASE() AND tc.constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY', 'UNIQUE')
GROUP BY tc.table_schema, tc.table_name, tc.constraint_name, tc.constraint_type
ORDER BY tc.table_name, tc.constraint_name`,
    indexes: `/* erd:indexes */
SELECT table_schema AS table_schema, table_name AS table_name, index_name AS index_name,
  CASE WHEN MAX(non_unique) = 0 THEN 'YES' ELSE 'NO' END AS is_unique,
  CASE WHEN index_name = 'PRIMARY' THEN 'YES' ELSE 'NO' END AS is_primary,
  GROUP_CONCAT(COALESCE(column_name, '(expression)') ORDER BY seq_in_index SEPARATOR '${US}') AS columns,
  MAX(index_type) AS method, 'NO' AS is_partial
FROM information_schema.statistics
WHERE table_schema = DATABASE()
GROUP BY table_schema, table_name, index_name
ORDER BY table_name, index_name`,
  },
  sqlite: {
    columns: `/* erd:columns */
SELECT 'main' AS table_schema, m.name AS table_name, m.type AS table_type, p.name AS column_name, p.type AS data_type,
  CASE WHEN p."notnull" = 1 OR p.pk > 0 THEN 'NO' ELSE 'YES' END AS is_nullable,
  COALESCE(p.dflt_value, '') AS column_default, p.cid + 1 AS ordinal, '' AS row_estimate, '' AS table_comment, '' AS column_comment
FROM sqlite_master m JOIN pragma_table_info(m.name) p
WHERE m.type IN ('table', 'view') AND m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
ORDER BY m.name, p.cid`,
    constraints: `/* erd:constraints */
SELECT 'main' AS table_schema, t AS table_name, cname AS constraint_name, ctype AS constraint_type, group_concat(col, '${US}') AS columns,
  rs AS ref_schema, rt AS ref_table, group_concat(rc, '${US}') AS ref_columns, od AS on_delete, ou AS on_update
FROM (
  SELECT m.name AS t, 'pk_' || m.name AS cname, 'p' AS ctype, p.name AS col, '' AS rs, '' AS rt, '' AS rc, '' AS od, '' AS ou, p.pk AS seq
  FROM sqlite_master m JOIN pragma_table_info(m.name) p WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND p.pk > 0
  UNION ALL
  SELECT m.name, 'fk_' || m.name || '_' || f.id, 'f', f."from", 'main', f."table", COALESCE(f."to", ''), f.on_delete, f.on_update, f.seq
  FROM sqlite_master m JOIN pragma_foreign_key_list(m.name) f WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
  UNION ALL
  SELECT m.name, il.name, 'u', ii.name, '', '', '', '', '', ii.seqno
  FROM sqlite_master m JOIN pragma_index_list(m.name) il JOIN pragma_index_info(il.name) ii
  WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND il."unique" = 1 AND il.origin = 'u'
  ORDER BY 1, 2, 10
)
GROUP BY t, cname, ctype, rs, rt, od, ou
ORDER BY t, cname`,
    indexes: `/* erd:indexes */
SELECT 'main' AS table_schema, t AS table_name, idx AS index_name, uq AS is_unique, pr AS is_primary, group_concat(col, '${US}') AS columns, '' AS method, part AS is_partial
FROM (
  SELECT m.name AS t, il.name AS idx, CASE WHEN il."unique" = 1 THEN 'YES' ELSE 'NO' END AS uq, CASE WHEN il.origin = 'pk' THEN 'YES' ELSE 'NO' END AS pr,
    COALESCE(ii.name, '(expression)') AS col, CASE WHEN il.partial = 1 THEN 'YES' ELSE 'NO' END AS part, ii.seqno AS seq
  FROM sqlite_master m JOIN pragma_index_list(m.name) il JOIN pragma_index_info(il.name) ii
  WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
  ORDER BY m.name, il.name, ii.seqno
)
GROUP BY t, idx, uq, pr, part
ORDER BY t, idx`,
  },
};

// SQL Server: sqlcmd has no CSV mode, so each row is built as one CSV line in SQL (every field
// quoted, quotes doubled) and the header is a first result set of its own. Needs SQL Server 2017+
// (STRING_AGG).
const MS_KEY = (rows, where) => `(SELECT STRING_AGG(c.name, N'${US}') WITHIN GROUP (ORDER BY ${rows}) ${where})`;
const MS_DESC = (major, minor) => `(SELECT CAST(ep.value AS nvarchar(max)) FROM sys.extended_properties ep WHERE ep.class = 1 AND ep.major_id = ${major} AND ep.minor_id = ${minor} AND ep.name = N'MS_Description')`;
const MS_PART = {
  columns: {
    cols: ["table_schema", "table_name", "table_type", "column_name", "data_type", "is_nullable", "column_default", "ordinal", "row_estimate", "table_comment", "column_comment"],
    order: "table_schema, table_name, ordinal",
    sql: `SELECT s.name AS table_schema, o.name AS table_name,
  CASE o.type WHEN 'V' THEN 'view' ELSE 'table' END AS table_type,
  c.name AS column_name,
  CASE
    WHEN t.name IN ('varchar', 'char', 'varbinary', 'binary') THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length AS varchar(10)) END + ')'
    WHEN t.name IN ('nvarchar', 'nchar') THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length / 2 AS varchar(10)) END + ')'
    WHEN t.name IN ('decimal', 'numeric') THEN t.name + '(' + CAST(c.precision AS varchar(10)) + ',' + CAST(c.scale AS varchar(10)) + ')'
    WHEN t.name IN ('datetime2', 'time', 'datetimeoffset') THEN t.name + '(' + CAST(c.scale AS varchar(10)) + ')'
    ELSE t.name END AS data_type,
  CASE WHEN c.is_nullable = 1 THEN 'YES' ELSE 'NO' END AS is_nullable,
  COALESCE(dc.definition, CASE WHEN c.is_identity = 1 THEN 'identity' END, 'AS ' + cc.definition, '') AS column_default,
  c.column_id AS ordinal,
  CASE WHEN o.type = 'U' THEN (SELECT CAST(SUM(p.rows) AS varchar(20)) FROM sys.partitions p WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)) ELSE '' END AS row_estimate,
  ${MS_DESC("o.object_id", "0")} AS table_comment,
  ${MS_DESC("o.object_id", "c.column_id")} AS column_comment
FROM sys.objects o
JOIN sys.schemas s ON s.schema_id = o.schema_id
JOIN sys.columns c ON c.object_id = o.object_id
JOIN sys.types t ON t.user_type_id = c.user_type_id
LEFT JOIN sys.default_constraints dc ON dc.object_id = c.default_object_id
LEFT JOIN sys.computed_columns cc ON cc.object_id = c.object_id AND cc.column_id = c.column_id
WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0`,
  },
  constraints: {
    cols: ["table_schema", "table_name", "constraint_name", "constraint_type", "columns", "ref_schema", "ref_table", "ref_columns", "on_delete", "on_update"],
    order: "table_schema, table_name, constraint_name",
    sql: `SELECT s.name AS table_schema, t.name AS table_name, kc.name AS constraint_name, CASE kc.type WHEN 'PK' THEN 'p' ELSE 'u' END AS constraint_type,
  ${MS_KEY("ic.key_ordinal", "FROM sys.index_columns ic JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id WHERE ic.object_id = kc.parent_object_id AND ic.index_id = kc.unique_index_id AND ic.is_included_column = 0")} AS columns,
  '' AS ref_schema, '' AS ref_table, '' AS ref_columns, '' AS on_delete, '' AS on_update
FROM sys.key_constraints kc
JOIN sys.tables t ON t.object_id = kc.parent_object_id
JOIN sys.schemas s ON s.schema_id = t.schema_id
WHERE t.is_ms_shipped = 0
UNION ALL
SELECT s.name, t.name, fk.name, 'f',
  ${MS_KEY("fkc.constraint_column_id", "FROM sys.foreign_key_columns fkc JOIN sys.columns c ON c.object_id = fkc.parent_object_id AND c.column_id = fkc.parent_column_id WHERE fkc.constraint_object_id = fk.object_id")},
  rs.name, rt.name,
  ${MS_KEY("fkc.constraint_column_id", "FROM sys.foreign_key_columns fkc JOIN sys.columns c ON c.object_id = fkc.referenced_object_id AND c.column_id = fkc.referenced_column_id WHERE fkc.constraint_object_id = fk.object_id")},
  REPLACE(fk.delete_referential_action_desc, '_', ' '), REPLACE(fk.update_referential_action_desc, '_', ' ')
FROM sys.foreign_keys fk
JOIN sys.tables t ON t.object_id = fk.parent_object_id
JOIN sys.schemas s ON s.schema_id = t.schema_id
JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id
JOIN sys.schemas rs ON rs.schema_id = rt.schema_id
WHERE t.is_ms_shipped = 0`,
  },
  indexes: {
    cols: ["table_schema", "table_name", "index_name", "is_unique", "is_primary", "columns", "method", "is_partial"],
    order: "table_schema, table_name, index_name",
    sql: `SELECT s.name AS table_schema, t.name AS table_name, i.name AS index_name,
  CASE WHEN i.is_unique = 1 THEN 'YES' ELSE 'NO' END AS is_unique,
  CASE WHEN i.is_primary_key = 1 THEN 'YES' ELSE 'NO' END AS is_primary,
  ${MS_KEY("ic.key_ordinal", "FROM sys.index_columns ic JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 0")} AS columns,
  LOWER(i.type_desc) AS method,
  CASE WHEN i.has_filter = 1 THEN 'YES' ELSE 'NO' END AS is_partial
FROM sys.indexes i
JOIN sys.tables t ON t.object_id = i.object_id
JOIN sys.schemas s ON s.schema_id = t.schema_id
WHERE i.type > 0 AND i.is_hypothetical = 0 AND t.is_ms_shipped = 0`,
  },
};

/** Wraps a SQL Server query so sqlcmd prints a header line, then one CSV line per row. */
function msCsv(tag, { cols, order, sql }) {
  // Catalog names and descriptions have their own collations; CONCAT needs one.
  const field = (c) => `REPLACE(CAST(q.${c} AS nvarchar(max)) COLLATE DATABASE_DEFAULT, N'"', N'""')`;
  const line = `CONCAT(N'"', ${cols.map(field).join(`, N'","', `)}, N'"')`;
  return `/* erd:${tag} */
SET NOCOUNT ON;
SELECT CAST(N'${cols.join(",")}' AS nvarchar(max));
SELECT ${line} FROM (
${sql}
) q
ORDER BY ${order};`;
}

SQL.mssql = Object.fromEntries(Object.entries(MS_PART).map(([tag, part]) => [tag, msCsv(tag, part)]));

/** The three catalog queries for a dialect: `{ columns, constraints, indexes }`. */
export function introspectionSql(kind) {
  const sql = SQL[kind];
  if (!sql) throw new Error(`Unknown database kind “${kind}”.`);
  return sql;
}

// Parsing CLI output ------------------------------------------------------------------------------

/** The parts of a database URL: `scheme://user:password@host:port/database?params`. */
export function parseDbUrl(text) {
  const m = /^([a-z][\w+.-]*):\/\/(.*)$/i.exec(String(text).trim());
  if (!m) return null;
  const scheme = m[1].toLowerCase();
  const kind = /^postgres(ql)?$/.test(scheme) ? "postgres" : /^(mysql|mariadb)$/.test(scheme) ? "mysql" : /^(sqlserver|mssql)$/.test(scheme) ? "mssql" : null;
  const decode = (s) => {
    try {
      return decodeURIComponent(s);
    } catch {
      return s;
    }
  };
  let rest = m[2];
  const q = rest.search(/[?#]/);
  const params = {};
  if (q >= 0) {
    for (const pair of rest.slice(q + 1).replace(/#.*$/, "").split(/[&;]/)) {
      const eq = pair.indexOf("=");
      if (eq > 0) params[decode(pair.slice(0, eq)).toLowerCase()] = decode(pair.slice(eq + 1));
    }
    rest = rest.slice(0, q);
  }
  const at = rest.lastIndexOf("@");
  const auth = at < 0 ? "" : rest.slice(0, at);
  const after = at < 0 ? rest : rest.slice(at + 1);
  const slash = after.indexOf("/");
  const hostPort = slash < 0 ? after : after.slice(0, slash);
  const hp = /^(\[[^\]]*\]|[^:]*)(?::(\d+))?$/.exec(hostPort) ?? [hostPort, hostPort, ""];
  const colon = auth.indexOf(":");
  return {
    kind,
    user: decode(colon < 0 ? auth : auth.slice(0, colon)),
    password: colon < 0 ? "" : decode(auth.slice(colon + 1)),
    host: hp[1].replace(/^\[|\]$/g, ""),
    port: hp[2] ? Number(hp[2]) : null,
    database: slash < 0 ? "" : decode(after.slice(slash + 1)),
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

/** `buildCommand`'s output as `{ columns, rows }`. */
export function parseResult(output, kind) {
  const text = String(output).replace(/\r\n/g, "\n");
  const mark = text.lastIndexOf(ROWS_MARK);
  const body = (mark < 0 ? text : text.slice(0, mark)).replace(/\n+$/, "");
  if (!body.trim()) return { columns: [], rows: [] };
  // sqlcmd may pad lines and separate result sets with blank lines.
  const all = kind === "mysql" ? parseTsv(body) : parseCsv(kind === "mssql" ? body.replace(/[ \t]+$/gm, "") : body);
  const [columns = [], ...rows] = kind === "mssql" ? all.filter((r) => r.length > 1 || r[0] !== "") : all;
  return { columns: columns.map((c) => c.trim().toLowerCase()), rows };
}

/** The CLI's error message without the row marker. */
export function errorText(output) {
  const text = String(output);
  const mark = text.lastIndexOf(ROWS_MARK);
  return (mark < 0 ? text : text.slice(0, mark)).trim() || "The query failed without a message.";
}

const shq = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;
const psq = (s) => `'${String(s).replace(/'/g, "''")}'`;
const winArg = (s) => (/^[\w./:@=+-]+$/.test(s) ? s : `"${String(s).replace(/(\\*)"/g, '$1$1\\"').replace(/(\\+)$/, "$1$1")}"`);

function delimiter(sql) {
  let d = "WINFER_SQL";
  while (sql.includes(d)) d += "_X";
  return d;
}

/**
 * The shell command that runs `sql` on a connection in a read-only session and prints its output,
 * then `@@WINFER_ROWS <lines>`, exiting with the CLI's code. `conn` is `{ key, meta }` from the vault.
 */
export function buildCommand(conn, sql, { timeoutSec = 90, win = IS_WIN } = {}) {
  const { key, meta } = conn;
  const ms = Math.max(1, timeoutSec - 2) * 1000;
  let program;
  let args;
  let env = [];
  if (meta.kind === "postgres") {
    program = "psql";
    args = [{ secret: key }, "-X", "-q", "--csv", "-v", "ON_ERROR_STOP=1"];
    env = [
      ["PGOPTIONS", `-c default_transaction_read_only=on -c statement_timeout=${ms}`],
      ["PGCONNECT_TIMEOUT", "10"],
      ["PGAPPNAME", "winfer-schema-diagram"],
    ];
  } else if (meta.kind === "mysql") {
    program = "mysql";
    args = ["--batch", "--default-character-set=utf8mb4", "--connect-timeout=10", "--init-command=SET SESSION TRANSACTION READ ONLY"];
    if (meta.host) args.push("-h", meta.host);
    if (meta.port) args.push("-P", String(meta.port));
    if (meta.user) args.push("-u", meta.user);
    if (meta.database) args.push(meta.database);
    if (meta.hasPassword) env.push(["MYSQL_PWD", { secret: key }]);
  } else if (meta.kind === "mssql") {
    // -b: exit with an error code; -h -1: no column headers; -y 0: long values in full.
    program = "sqlcmd";
    args = ["-S", `tcp:${meta.host || "localhost"},${meta.port || 1433}`, "-b", "-h", "-1", "-y", "0", "-l", "10", "-K", "ReadOnly"];
    if (meta.database) args.push("-d", meta.database);
    if (meta.user) args.push("-U", meta.user);
    else args.push("-E");
    if (meta.trustCert) args.push("-C");
    if (win) args.push("-f", "65001");
    if (meta.hasPassword) env.push(["SQLCMDPASSWORD", { secret: key }]);
  } else if (meta.kind === "sqlite") {
    program = "sqlite3";
    args = ["-readonly", "-safe", "-bail", "-csv", "-header", "-cmd", ".timeout 5000", meta.path];
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
      'cat "$t"',
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
    "Get-Content $o.FullName -Encoding UTF8",
    "Get-Content $e.FullName -Encoding UTF8",
    `"\`n${ROWS_MARK} " + (Get-Content $o.FullName | Measure-Object -Line).Lines`,
    "Remove-Item $f.FullName, $o.FullName, $e.FullName",
    "exit $p.ExitCode",
  ].join("\n");
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

// The model ---------------------------------------------------------------------------------------

/** A `{ columns, rows }` result as objects keyed by column name. */
function records(result) {
  if (!result) return [];
  return result.rows.map((row) => Object.fromEntries(result.columns.map((c, i) => [c, row[i] ?? ""])));
}

const keyList = (cell) => (cell ? String(cell).split(US) : []);
const sameSet = (a, b) => a.length === b.length && a.every((x) => b.includes(x));
const tableId = (schema, name) => `${schema}.${name}`;

/**
 * Builds the schema model from the three catalog results:
 * `{ kind, schemas, tables: [{ id, schema, name, kind, rows, comment, columns, primaryKey, uniques,
 * indexes, junction }], relations: [{ id, name, from, fromColumns, to, toColumns, onDelete, onUpdate,
 * inferred, one, optional, identifying }] }`. `one` means each row of `to` has at most one row of
 * `from` (a 1:1); `optional` that a `from` row may have no `to` row (a nullable key).
 */
export function buildModel(kind, results) {
  const tables = new Map();
  for (const r of records(results.columns)) {
    const id = tableId(r.table_schema, r.table_name);
    if (!tables.has(id)) {
      const rows = r.row_estimate === "" ? null : Number(r.row_estimate);
      tables.set(id, {
        id,
        schema: r.table_schema,
        name: r.table_name,
        kind: r.table_type || "table",
        rows: Number.isFinite(rows) ? rows : null,
        comment: r.table_comment || "",
        columns: [],
        primaryKey: [],
        uniques: [],
        indexes: [],
        junction: null,
      });
    }
    tables.get(id).columns.push({
      name: r.column_name,
      type: r.data_type || "",
      nullable: r.is_nullable !== "NO",
      default: r.column_default || "",
      comment: r.column_comment || "",
      pk: false,
      fk: false,
      unique: false,
      indexed: false,
    });
  }

  const stub = (schema, name, columns) => {
    const id = tableId(schema, name);
    if (!tables.has(id)) {
      tables.set(id, { id, schema, name, kind: "external", rows: null, comment: "", columns: columns.map((c) => ({ name: c, type: "", nullable: false, default: "", comment: "", pk: false, fk: false, unique: false, indexed: false })), primaryKey: [], uniques: [], indexes: [], junction: null });
    }
    return tables.get(id);
  };

  const fks = [];
  for (const r of records(results.constraints)) {
    const t = tables.get(tableId(r.table_schema, r.table_name));
    if (!t) continue;
    const cols = keyList(r.columns);
    if (r.constraint_type === "p") t.primaryKey = cols;
    else if (r.constraint_type === "u") t.uniques.push(cols);
    else if (r.constraint_type === "f") fks.push({ t, r, cols });
  }

  for (const r of records(results.indexes)) {
    const t = tables.get(tableId(r.table_schema, r.table_name));
    if (!t) continue;
    const index = { name: r.index_name, unique: r.is_unique === "YES", primary: r.is_primary === "YES", columns: keyList(r.columns), method: r.method || "", partial: r.is_partial === "YES" };
    t.indexes.push(index);
    if (index.unique && !index.primary && !index.partial && !t.uniques.some((u) => sameSet(u, index.columns))) t.uniques.push(index.columns);
    // SQLite has no named primary key constraint for rowid tables, only the pk flags.
    if (index.primary && t.primaryKey.length === 0) t.primaryKey = index.columns;
  }

  const relations = [];
  for (const { t, r, cols } of fks) {
    const refSchema = r.ref_schema || t.schema;
    let target = tables.get(tableId(refSchema, r.ref_table));
    let toColumns = keyList(r.ref_columns).filter(Boolean);
    if (!target) target = stub(refSchema, r.ref_table, toColumns);
    if (toColumns.length !== cols.length) toColumns = target.primaryKey.length === cols.length ? target.primaryKey : cols.map((_, i) => target.primaryKey[i] ?? target.columns[i]?.name ?? "?");
    relations.push(relation(t, target, r.constraint_name, cols, toColumns, { onDelete: r.on_delete, onUpdate: r.on_update }));
  }

  for (const t of tables.values()) {
    const indexed = new Set(t.indexes.map((ix) => ix.columns[0]));
    for (const c of t.columns) {
      c.pk = t.primaryKey.includes(c.name);
      c.unique = c.pk ? t.primaryKey.length === 1 : t.uniques.some((u) => u.length === 1 && u[0] === c.name);
      c.indexed = indexed.has(c.name);
    }
  }
  for (const rel of relations) {
    const t = tables.get(rel.from);
    for (const c of t.columns) if (rel.fromColumns.includes(c.name)) c.fk = true;
  }

  const model = { kind, schemas: [...new Set([...tables.values()].map((t) => t.schema))].sort(), tables: [...tables.values()], relations };
  markJunctions(model);
  return model;
}

/** A relation from `from`'s columns to `to`'s, with its cardinality worked out from the keys. */
function relation(from, to, name, fromColumns, toColumns, { onDelete = "", onUpdate = "", inferred = false } = {}) {
  const cols = fromColumns.map((n) => from.columns.find((c) => c.name === n));
  const one = (from.primaryKey.length > 0 && sameSet(from.primaryKey, fromColumns)) || from.uniques.some((u) => sameSet(u, fromColumns));
  return {
    id: `${from.id}:${name}`,
    name,
    from: from.id,
    fromColumns,
    to: to.id,
    toColumns,
    onDelete: onDelete === "NO ACTION" ? "" : onDelete,
    onUpdate: onUpdate === "NO ACTION" ? "" : onUpdate,
    inferred,
    one,
    optional: cols.some((c) => c?.nullable),
    identifying: fromColumns.length > 0 && fromColumns.every((n) => from.primaryKey.includes(n)),
  };
}

/**
 * Marks join tables, which make a many-to-many between two others: exactly two foreign keys that
 * are together the primary key (or unique), and at most two other columns besides the key.
 */
export function markJunctions(model) {
  for (const t of model.tables) {
    t.junction = null;
    if (t.kind !== "table") continue;
    const out = model.relations.filter((r) => r.from === t.id && !r.inferred);
    if (out.length !== 2) continue;
    const keyCols = [...new Set(out.flatMap((r) => r.fromColumns))];
    const keyed = (t.primaryKey.length > 0 && t.primaryKey.every((c) => keyCols.includes(c))) || t.uniques.some((u) => sameSet(u, keyCols));
    const others = t.columns.filter((c) => !keyCols.includes(c.name) && !c.pk);
    if (keyed && others.length <= 2) t.junction = [out[0].to, out[1].to];
  }
  return model;
}

/** Name variants a table a column like `customer_id` might point at could have. */
function targetNames(base) {
  const b = base.toLowerCase().replace(/[^a-z0-9]/g, "");
  if (!b) return [];
  const out = [b, `${b}s`, `${b}es`];
  if (b.endsWith("y")) out.push(`${b.slice(0, -1)}ies`);
  if (b.endsWith("s")) out.push(b.slice(0, -1));
  return out;
}

const SELF = new Set(["parent", "parentid", "manager", "reportsto", "supervisor", "predecessor", "successor", "replyto", "previous", "next"]);

/**
 * Relations the database doesn't declare but the names suggest: a column `customer_id`
 * (or `customerId`) without a foreign key, in a table, pointing at the single-column primary key
 * of a table called customer(s), or at its own table for `parent_id` and the like. Returns new
 * relations marked `inferred`.
 */
export function inferRelations(model) {
  const byName = new Map();
  for (const t of model.tables) {
    if (t.kind === "view" || t.primaryKey.length !== 1) continue;
    const key = t.name.toLowerCase().replace(/[^a-z0-9]/g, "");
    if (!byName.has(key)) byName.set(key, []);
    byName.get(key).push(t);
  }
  const out = [];
  for (const t of model.tables) {
    if (t.kind !== "table") continue;
    for (const c of t.columns) {
      if (c.fk || (c.pk && t.primaryKey.length === 1)) continue;
      const m = /^(.+?)_?(?:id|Id|ID)$/.exec(c.name);
      if (!m || /_$/.test(m[1])) continue;
      const base = m[1];
      let target = null;
      if (SELF.has(base.toLowerCase().replace(/[^a-z]/g, "")) && t.primaryKey.length === 1) target = t;
      for (const name of targetNames(base)) {
        if (target) break;
        const hits = byName.get(name) ?? [];
        target = hits.find((x) => x.schema === t.schema) ?? hits[0] ?? null;
      }
      if (!target) continue;
      const rel = relation(t, target, `${c.name} (inferred)`, [c.name], target.primaryKey, { inferred: true });
      if (!model.relations.some((r) => r.from === rel.from && r.to === rel.to && sameSet(r.fromColumns, rel.fromColumns))) out.push(rel);
    }
  }
  return out;
}

/** Counts for the page and for agents. */
export function stats(model, inferred = []) {
  const tables = model.tables.filter((t) => t.kind === "table").length;
  const views = model.tables.filter((t) => t.kind === "view" || t.kind === "materialized view").length;
  return {
    tables,
    views,
    columns: model.tables.reduce((n, t) => n + t.columns.length, 0),
    relations: model.relations.length,
    inferred: inferred.length,
    junctions: model.tables.filter((t) => t.junction).length,
    isolated: model.tables.filter((t) => t.kind !== "external" && ![...model.relations, ...inferred].some((r) => r.from === t.id || r.to === t.id)).length,
    noPrimaryKey: model.tables.filter((t) => t.kind === "table" && t.primaryKey.length === 0).map((t) => t.id),
  };
}

/** Foreign keys whose columns no index starts with: joins and cascades on them scan the table. */
export function unindexedForeignKeys(model) {
  const out = [];
  for (const rel of model.relations) {
    if (rel.inferred) continue;
    const t = model.tables.find((x) => x.id === rel.from);
    const covered = t.indexes.some((ix) => rel.fromColumns.every((c, i) => ix.columns[i] === c)) || rel.fromColumns.every((c, i) => t.primaryKey[i] === c);
    if (!covered) out.push(rel);
  }
  return out;
}

// Layout ------------------------------------------------------------------------------------------

export const METRICS = { header: 32, stereo: 12, row: 20, pad: 8, minWidth: 170, maxWidth: 380, gapX: 120, gapY: 36, gapComp: 90 };

/** The columns a box shows: "all", "keys" (primary, foreign and unique keys) or "none". */
export function visibleColumns(table, mode) {
  if (mode === "none") return [];
  if (mode === "keys") return table.columns.filter((c) => c.pk || c.fk || c.unique);
  return table.columns;
}

const estimate = (text, font) => String(text).length * (font === "mono" ? 6.9 : 7.6);

/** A box's `{ w, h }`. `measure(text, "mono" | "sans")` gives text widths. */
export function measureTable(table, mode, measure = estimate) {
  const cols = visibleColumns(table, mode);
  let w = Math.max(METRICS.minWidth, measure(table.name, "sans") + 44);
  for (const c of cols) w = Math.max(w, 30 + measure(c.name, "mono") + 18 + measure(shortType(c.type), "mono") + 12);
  const stereo = stereotype(table) ? METRICS.stereo : 0;
  return { w: Math.ceil(Math.min(METRICS.maxWidth, w)), h: METRICS.header + stereo + cols.length * METRICS.row + (cols.length ? METRICS.pad : 0) };
}

/** Shorter spellings of long Postgres type names. */
export function shortType(type) {
  return String(type)
    .replace(/\btimestamp(\(\d+\))? with time zone\b/g, "timestamptz$1")
    .replace(/\btimestamp(\(\d+\))? without time zone\b/g, "timestamp$1")
    .replace(/\btime(\(\d+\))? with time zone\b/g, "timetz$1")
    .replace(/\btime(\(\d+\))? without time zone\b/g, "time$1")
    .replace(/\bcharacter varying\b/g, "varchar")
    .replace(/\bdouble precision\b/g, "float8")
    .replace(/\bcharacter\b/g, "char");
}

export function stereotype(table) {
  if (table.kind === "view") return "«view»";
  if (table.kind === "materialized view") return "«materialized view»";
  if (table.kind === "foreign table") return "«foreign table»";
  if (table.kind === "external") return "«external»";
  if (table.junction) return "«join table»";
  return "";
}

/**
 * Places the boxes: each group of related tables in columns, referenced tables to the left of the
 * tables referencing them, ordered to cross few lines; groups and lone tables packed in rows.
 * `sizes` maps table id → `{ w, h }`; returns a Map of id → `{ x, y }`.
 */
export function layout(tables, relations, sizes) {
  const ids = tables.map((t) => t.id);
  const known = new Set(ids);
  const parents = new Map(ids.map((id) => [id, new Set()]));
  const neighbours = new Map(ids.map((id) => [id, new Set()]));
  for (const r of relations) {
    if (r.from === r.to || !known.has(r.from) || !known.has(r.to)) continue;
    parents.get(r.from).add(r.to);
    neighbours.get(r.from).add(r.to);
    neighbours.get(r.to).add(r.from);
  }

  // Connected groups.
  const group = new Map();
  const groups = [];
  for (const id of ids) {
    if (group.has(id)) continue;
    const members = [];
    const stack = [id];
    group.set(id, groups.length);
    while (stack.length) {
      const n = stack.pop();
      members.push(n);
      for (const m of neighbours.get(n)) {
        if (!group.has(m)) {
          group.set(m, groups.length);
          stack.push(m);
        }
      }
    }
    groups.push(members);
  }

  const blocks = groups.map((members) => placeGroup(members, parents, neighbours, sizes));
  // Big groups first, lone tables last, then by name for a stable picture.
  blocks.sort((a, b) => b.members.length - a.members.length || b.h - a.h || a.members[0].localeCompare(b.members[0]));
  const area = blocks.reduce((n, b) => n + (b.w + METRICS.gapComp) * (b.h + METRICS.gapComp), 0);
  const rowWidth = Math.max(blocks[0]?.w ?? 0, Math.sqrt(area) * 1.5);

  const out = new Map();
  let x = 0;
  let y = 0;
  let rowH = 0;
  for (const b of blocks) {
    if (x > 0 && x + b.w > rowWidth) {
      x = 0;
      y += rowH + METRICS.gapComp;
      rowH = 0;
    }
    for (const [id, p] of b.pos) out.set(id, { x: x + p.x, y: y + p.y });
    x += b.w + METRICS.gapComp;
    rowH = Math.max(rowH, b.h);
  }
  return out;
}

function placeGroup(members, parents, neighbours, sizes) {
  const inGroup = new Set(members);
  // Rank: one more than the deepest table it references (cycles are cut where they're found).
  const rank = new Map();
  const visiting = new Set();
  const rankOf = (id) => {
    if (rank.has(id)) return rank.get(id);
    if (visiting.has(id)) return 0;
    visiting.add(id);
    let r = 0;
    for (const p of parents.get(id)) if (inGroup.has(p)) r = Math.max(r, rankOf(p) + 1);
    visiting.delete(id);
    rank.set(id, r);
    return r;
  };
  members.forEach(rankOf);
  // Pull tables nothing references next to the tables they reference... they already are; push
  // referenced-only tables right next to their first child instead of far left.
  for (const id of [...members].sort((a, b) => rank.get(b) - rank.get(a))) {
    const children = members.filter((m) => parents.get(m).has(id));
    if (children.length && parents.get(id).size === 0) rank.set(id, Math.max(0, Math.min(...children.map((c) => rank.get(c))) - 1));
  }

  const layers = [];
  for (const id of members) (layers[rank.get(id)] ??= []).push(id);
  const columns = layers.filter(Boolean).map((l) => l.sort((a, b) => neighbours.get(b).size - neighbours.get(a).size || a.localeCompare(b)));

  // Barycenter sweeps to cut crossings.
  const index = new Map();
  const reindex = () => columns.forEach((col) => col.forEach((id, i) => index.set(id, i / Math.max(1, col.length - 1))));
  reindex();
  for (let sweep = 0; sweep < 8; sweep++) {
    const order = sweep % 2 === 0 ? columns.map((_, i) => i) : columns.map((_, i) => columns.length - 1 - i);
    for (const ci of order) {
      const col = columns[ci];
      const bary = new Map(
        col.map((id) => {
          const ns = [...neighbours.get(id)].filter((n) => inGroup.has(n) && !col.includes(n));
          return [id, ns.length ? ns.reduce((s, n) => s + index.get(n), 0) / ns.length : index.get(id)];
        }),
      );
      col.sort((a, b) => bary.get(a) - bary.get(b));
      col.forEach((id, i) => index.set(id, i / Math.max(1, col.length - 1)));
    }
  }

  // Very tall columns wrap into several.
  const total = members.reduce((n, id) => n + sizes.get(id).h + METRICS.gapY, 0);
  const maxH = Math.max(900, Math.sqrt(total * 1400));
  const wrapped = [];
  for (const col of columns) {
    let cur = [];
    let h = 0;
    for (const id of col) {
      const dh = sizes.get(id).h + METRICS.gapY;
      if (cur.length && h + dh > maxH) {
        wrapped.push(cur);
        cur = [];
        h = 0;
      }
      cur.push(id);
      h += dh;
    }
    if (cur.length) wrapped.push(cur);
  }

  // x by column; y lines a table up with what it's connected to on its left, without overlaps.
  const pos = new Map();
  let x = 0;
  for (const col of wrapped) {
    const w = Math.max(...col.map((id) => sizes.get(id).w));
    const wanted = col.map((id) => {
      const ys = [...neighbours.get(id)].filter((n) => pos.has(n)).map((n) => pos.get(n).y + sizes.get(n).h / 2);
      return ys.length ? ys.reduce((a, b) => a + b, 0) / ys.length - sizes.get(id).h / 2 : null;
    });
    let y = 0;
    let floor = -Infinity;
    col.forEach((id, i) => {
      const target = wanted[i] ?? (Number.isFinite(floor) ? floor : y);
      const top = Math.max(target, floor === -Infinity ? target : floor);
      pos.set(id, { x, y: top });
      floor = top + sizes.get(id).h + METRICS.gapY;
      y = floor;
    });
    x += w + METRICS.gapX;
  }
  const minY = Math.min(...[...pos.values()].map((p) => p.y));
  let h = 0;
  for (const [id, p] of pos) {
    p.y -= minY;
    h = Math.max(h, p.y + sizes.get(id).h);
  }
  return { members, pos, w: x - METRICS.gapX, h };
}

// Exports -----------------------------------------------------------------------------------------

const mermaidId = (t, multi) => (multi ? `${t.schema}_${t.name}` : t.name).replace(/[^A-Za-z0-9_]/g, "_");
const mermaidType = (type) => shortType(type).replace(/\s+/g, "_").replace(/,/g, "-").replace(/[^A-Za-z0-9_()[\]-]/g, "") || "unknown";

/** A Mermaid `erDiagram` of the tables (all of them unless `ids` is given). */
export function toMermaidEr(model, relations = model.relations, ids = null) {
  const shown = model.tables.filter((t) => !ids || ids.has(t.id));
  const keep = new Set(shown.map((t) => t.id));
  const multi = new Set(shown.map((t) => t.schema)).size > 1;
  const byId = new Map(model.tables.map((t) => [t.id, t]));
  const lines = ["erDiagram"];
  for (const r of relations) {
    if (!keep.has(r.from) || !keep.has(r.to)) continue;
    const left = r.optional ? "|o" : "||";
    const right = r.one ? "o|" : "o{";
    const label = `${r.fromColumns.join(", ")}${r.inferred ? " (inferred)" : ""}`;
    lines.push(`  ${mermaidId(byId.get(r.to), multi)} ${left}${r.inferred ? ".." : "--"}${right} ${mermaidId(byId.get(r.from), multi)} : "${label.replace(/"/g, "'")}"`);
  }
  for (const t of shown) {
    lines.push(`  ${mermaidId(t, multi)} {`);
    for (const c of t.columns) {
      const keys = [c.pk && "PK", c.fk && "FK", !c.pk && c.unique && "UK"].filter(Boolean).join(", ");
      lines.push(`    ${mermaidType(c.type)} ${c.name.replace(/[^A-Za-z0-9_-]/g, "_")}${keys ? ` ${keys}` : ""}${c.nullable ? "" : ' "not null"'}`);
    }
    lines.push("  }");
  }
  return lines.join("\n");
}

/** A Mermaid `classDiagram`: tables as classes, foreign keys as associations with multiplicities. */
export function toMermaidClass(model, relations = model.relations, ids = null) {
  const shown = model.tables.filter((t) => !ids || ids.has(t.id));
  const keep = new Set(shown.map((t) => t.id));
  const multi = new Set(shown.map((t) => t.schema)).size > 1;
  const byId = new Map(model.tables.map((t) => [t.id, t]));
  const lines = ["classDiagram"];
  for (const t of shown) {
    lines.push(`  class ${mermaidId(t, multi)} {`);
    const st = stereotype(t);
    if (st) lines.push(`    <<${st.slice(1, -1)}>>`);
    // Parentheses would make a member a method: varchar(255) becomes varchar_255.
    for (const c of t.columns) lines.push(`    ${c.pk ? "+" : c.fk ? "#" : "-"}${mermaidType(c.type).replace(/[()[\]]+/g, "_").replace(/_$/, "")} ${c.name.replace(/[^A-Za-z0-9_-]/g, "_")}${c.nullable ? "?" : ""}`);
    lines.push("  }");
  }
  for (const r of relations) {
    if (!keep.has(r.from) || !keep.has(r.to)) continue;
    lines.push(`  ${mermaidId(byId.get(r.from), multi)} "${r.one ? "0..1" : "*"}" ${r.inferred ? ".." : "--"}> "${r.optional ? "0..1" : "1"}" ${mermaidId(byId.get(r.to), multi)} : ${r.fromColumns.join("_").replace(/[^A-Za-z0-9_]/g, "_")}`);
  }
  return lines.join("\n");
}

/** A PlantUML class diagram. */
export function toPlantUml(model, relations = model.relations, ids = null) {
  const shown = model.tables.filter((t) => !ids || ids.has(t.id));
  const keep = new Set(shown.map((t) => t.id));
  const multi = new Set(shown.map((t) => t.schema)).size > 1;
  const byId = new Map(model.tables.map((t) => [t.id, t]));
  const name = (t) => `"${multi ? `${t.schema}.` : ""}${t.name}"`;
  const lines = ["@startuml", "hide circle", "skinparam linetype ortho"];
  for (const t of shown) {
    const st = stereotype(t);
    lines.push(`class ${name(t)}${st ? ` <<${st.slice(1, -1)}>>` : ""} {`);
    for (const c of t.columns) lines.push(`  ${c.pk ? "{static} +" : c.fk ? "#" : "-"}${c.name} : ${shortType(c.type)}${c.nullable ? " [0..1]" : ""}`);
    lines.push("}");
  }
  for (const r of relations) {
    if (!keep.has(r.from) || !keep.has(r.to)) continue;
    lines.push(`${name(byId.get(r.from))} "${r.one ? "0..1" : "*"}" ${r.inferred ? ".." : "--"}> "${r.optional ? "0..1" : "1"}" ${name(byId.get(r.to))} : ${r.fromColumns.join(", ")}`);
  }
  lines.push("@enduml");
  return lines.join("\n");
}

const cardinality = (r) => `${r.optional ? "0..1" : "1"} → ${r.one ? "0..1" : "*"}`;

/** A plain-text description for agents: every table with its keys and relations, or one table in full. */
export function describeModel(model, relations, table = null) {
  const byId = new Map(model.tables.map((t) => [t.id, t]));
  const label = (id) => {
    const t = byId.get(id);
    return model.schemas.length > 1 ? `${t.schema}.${t.name}` : t.name;
  };
  const relText = (r) =>
    `${label(r.from)}(${r.fromColumns.join(", ")}) → ${label(r.to)}(${r.toColumns.join(", ")}) [${r.one ? "one-to-one" : "many-to-one"}${r.optional ? ", optional" : ""}${r.onDelete ? `, ON DELETE ${r.onDelete}` : ""}${r.inferred ? ", inferred from the name, no foreign key" : ""}]`;
  if (table) {
    const wanted = String(table).toLowerCase();
    const t = model.tables.find((x) => x.id.toLowerCase() === wanted || x.name.toLowerCase() === wanted);
    if (!t) throw new Error(`No table “${table}”. Tables: ${model.tables.map((x) => label(x.id)).join(", ")}`);
    const lines = [`${label(t.id)} (${t.kind}${t.rows !== null ? `, ~${t.rows.toLocaleString("en")} rows` : ""})${t.comment ? ` — ${t.comment}` : ""}`];
    if (t.junction) lines.push(`Join table: links ${label(t.junction[0])} and ${label(t.junction[1])} many-to-many.`);
    lines.push("", "Columns:");
    for (const c of t.columns) lines.push(`  ${c.name} ${c.type}${c.nullable ? "" : " NOT NULL"}${c.pk ? " PK" : ""}${c.fk ? " FK" : ""}${!c.pk && c.unique ? " UNIQUE" : ""}${c.default ? (/^(identity|auto_increment|AS )/.test(c.default) ? ` ${c.default}` : ` DEFAULT ${c.default}`) : ""}${c.comment ? ` — ${c.comment}` : ""}`);
    if (t.indexes.length) {
      lines.push("", "Indexes:");
      for (const ix of t.indexes) lines.push(`  ${ix.name} (${ix.columns.join(", ")})${ix.primary ? " primary" : ix.unique ? " unique" : ""}${ix.partial ? " partial" : ""}`);
    }
    const out = relations.filter((r) => r.from === t.id);
    const inc = relations.filter((r) => r.to === t.id && r.from !== t.id);
    if (out.length) lines.push("", "References:", ...out.map((r) => `  ${relText(r)}`));
    if (inc.length) lines.push("", "Referenced by:", ...inc.map((r) => `  ${relText(r)}`));
    return lines.join("\n");
  }
  const s = stats(model, relations.filter((r) => r.inferred));
  const lines = [`${s.tables} tables, ${s.views} views, ${s.relations} foreign keys${s.inferred ? `, ${s.inferred} inferred relations` : ""}.`, ""];
  for (const t of model.tables) {
    if (t.kind === "external") continue;
    const keys = t.columns.filter((c) => c.pk || c.fk).map((c) => `${c.name}${c.pk ? " PK" : ""}${c.fk ? " FK" : ""}`);
    lines.push(`${label(t.id)}${t.kind !== "table" ? ` (${t.kind})` : ""}${t.junction ? " (join table)" : ""}: ${t.columns.length} columns${keys.length ? `; ${keys.join(", ")}` : ""}`);
  }
  if (relations.length) lines.push("", "Relations:", ...relations.map((r) => `  ${relText(r)}`));
  if (s.noPrimaryKey.length) lines.push("", `Tables without a primary key: ${s.noPrimaryKey.map(label).join(", ")}`);
  const slow = unindexedForeignKeys(model);
  if (slow.length) lines.push("", `Foreign keys without an index: ${slow.map((r) => `${label(r.from)}(${r.fromColumns.join(", ")})`).join(", ")}`);
  return lines.join("\n");
}

/** The tables within `depth` relations of `start` (a Set of ids). */
export function neighbourhood(model, relations, start, depth = 1) {
  const seen = new Set([start]);
  let frontier = [start];
  for (let d = 0; d < depth; d++) {
    const next = [];
    for (const id of frontier) {
      for (const r of relations) {
        const other = r.from === id ? r.to : r.to === id ? r.from : null;
        if (other && !seen.has(other)) {
          seen.add(other);
          next.push(other);
        }
      }
    }
    frontier = next;
  }
  return seen;
}

// Plugin ------------------------------------------------------------------------------------------

const keyName = (text) => `ERD_${String(text).toUpperCase().replace(/[^A-Z0-9_]+/g, "_").replace(/^_+|_+$/g, "")}`;

function target(meta) {
  if (meta.kind === "sqlite") return meta.path;
  return `${meta.user ? `${meta.user}@` : ""}${meta.host || "localhost"}${meta.port ? `:${meta.port}` : ""}${meta.database ? `/${meta.database}` : ""}`;
}

function loadJson(key, fallback) {
  try {
    return JSON.parse(localStorage.getItem(key) ?? "null") ?? fallback;
  } catch {
    return fallback;
  }
}

function saveJson(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage full or off: it just isn't kept.
  }
}

function createStore(api) {
  /** Per connection key: { state: "loading" | "ok" | "error", model?, error?, at? }. */
  const schemas = new Map();
  const listeners = new Set();
  const emit = (event) => listeners.forEach((fn) => fn(event));
  let home = null;

  async function absolute(dir) {
    if (!dir || dir === "~") return "";
    if (!dir.startsWith("~/") && !dir.startsWith("~\\")) return dir;
    if (home === null) {
      const r = await api.vault.exec({ command: IS_WIN ? "echo %USERPROFILE%" : 'echo "$HOME"', cwd: "", keys: [], timeoutSec: 15 });
      home = r.output.trim();
    }
    return home + dir.slice(1);
  }

  async function connections() {
    return (await api.vault.list()).filter((s) => KINDS[s.meta?.kind]).sort((a, b) => String(a.meta.name || a.key).localeCompare(String(b.meta.name || b.key)));
  }

  async function find(name) {
    const all = await connections();
    const wanted = String(name ?? "").trim().toLowerCase();
    const hit = all.find((c) => c.key.toLowerCase() === wanted || String(c.meta.name ?? "").toLowerCase() === wanted || keyName(wanted).toLowerCase() === c.key.toLowerCase());
    if (hit) return hit;
    if (!wanted && all.length === 1) return all[0];
    throw new Error(`Unknown connection “${name}”. Connections: ${all.map((c) => c.key).join(", ") || "none yet"}. The user can add one on winfer's Schema Diagram page.`);
  }

  /** Runs one catalog query; resolves to `{ columns, rows }` or throws the CLI's message. */
  async function query(conn, sql, timeoutSec = 90) {
    const keys = conn.meta.kind === "postgres" || ((conn.meta.kind === "mysql" || conn.meta.kind === "mssql") && conn.meta.hasPassword) ? [conn.key] : [];
    const command = buildCommand(conn, sql, { timeoutSec });
    const r = await api.vault.exec({ command: IS_WIN ? powershell(command) : command, cwd: await absolute(conn.meta.base ?? ""), keys, timeoutSec });
    if (r.timedOut) throw new Error(`Reading the schema stopped after ${timeoutSec} s.`);
    if (r.code !== 0) {
      const text = errorText(r.output);
      const cli = KINDS[conn.meta.kind].cli;
      if (new RegExp(`(${cli}: (command )?not found|not recognized as|cannot find the file)`, "i").test(text)) throw new Error(`${cli} isn't installed or isn't on the PATH. Install the ${KINDS[conn.meta.kind].name} command-line client and try again.\n\n${text}`);
      if (conn.meta.kind === "mssql" && /certificate/i.test(text) && !conn.meta.trustCert) throw new Error(`The server's certificate isn't trusted. For a local or dev server, add ?trustServerCertificate=true to the URL.\n\n${text}`);
      throw new Error(text);
    }
    return parseResult(r.output, conn.meta.kind);
  }

  /** Reads a connection's schema (cached unless `fresh`); resolves to the model. */
  async function load(conn, { fresh = false } = {}) {
    const have = schemas.get(conn.key);
    if (have && !fresh) {
      if (have.state === "ok") return have.model;
      if (have.state === "loading") return have.promise;
    }
    const sql = introspectionSql(conn.meta.kind);
    const promise = (async () => {
      const [columns, constraints, indexes] = await Promise.all([query(conn, sql.columns), query(conn, sql.constraints), query(conn, sql.indexes)]);
      return buildModel(conn.meta.kind, { columns, constraints, indexes });
    })();
    schemas.set(conn.key, { state: "loading", promise });
    emit({ type: "schema", key: conn.key });
    try {
      const model = await promise;
      schemas.set(conn.key, { state: "ok", model, at: Date.now() });
      emit({ type: "schema", key: conn.key });
      return model;
    } catch (err) {
      schemas.set(conn.key, { state: "error", error: String(err?.message ?? err) });
      emit({ type: "schema", key: conn.key });
      throw err;
    }
  }

  return {
    connections,
    find,
    load,
    schema: (key) => schemas.get(key) ?? null,
    forget: (key) => schemas.delete(key),
    /** Asks the open page to show a table (from an agent or the palette). */
    focus: (key, table) => emit({ type: "focus", key, table }),
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

const prefs = () => ({ columns: "all", notation: "crow", inferred: true, ...loadJson(PREFS, {}) });

export function activate(api) {
  const store = createStore(api);
  let pending = null;

  const relationsFor = (model) => [...model.relations, ...(prefs().inferred ? inferRelations(model) : [])];

  api.registerAgentTool({
    name: "erd_list",
    description: "List the databases connected in winfer's Schema Diagram (Postgres, MySQL, SQLite, SQL Server), by name. Credentials are never shown. Use erd_describe or erd_diagram on one.",
    inputSchema: { type: "object", properties: {} },
    async run() {
      const all = await store.connections();
      if (all.length === 0) return "No databases connected yet. Ask the user to add one on winfer's Schema Diagram page.";
      return all.map((c) => `${c.key} (${c.meta.name || c.key}): ${KINDS[c.meta.kind].name} ${target(c.meta)}${c.meta.description ? ` — ${c.meta.description}` : ""}`).join("\n");
    },
  });

  api.registerAgentTool({
    name: "erd_describe",
    description: "Describe a database's structure: every table and view with its keys, and every relation (foreign keys and ones inferred from column names) with its cardinality. With `table`, that table's columns, indexes and what references it.",
    inputSchema: {
      type: "object",
      properties: {
        connection: { type: "string", description: "Connection name or key from erd_list" },
        table: { type: "string", description: "One table to describe in full (optional)" },
      },
      required: ["connection"],
    },
    async run(args) {
      const conn = await store.find(args.connection);
      const model = await store.load(conn);
      const text = describeModel(model, relationsFor(model), args.table || null);
      return text.length > AGENT_CHARS ? `${text.slice(0, AGENT_CHARS)}\n… cut; ask for one table at a time.` : text;
    },
  });

  api.registerAgentTool({
    name: "erd_diagram",
    description: "Get a database's schema as a diagram in Mermaid (erDiagram or classDiagram) or PlantUML, for docs or to reason about relations. Narrow it to the tables around one table with `table` and `depth`.",
    inputSchema: {
      type: "object",
      properties: {
        connection: { type: "string", description: "Connection name or key from erd_list" },
        format: { type: "string", enum: ["mermaid-er", "mermaid-class", "plantuml"], description: "Default mermaid-er" },
        table: { type: "string", description: "Only this table and its neighbours (optional)" },
        depth: { type: "number", description: "How many relations away from `table` to include (default 1)" },
        show: { type: "boolean", description: "Also open the diagram on that table for the user" },
      },
      required: ["connection"],
    },
    async run(args) {
      const conn = await store.find(args.connection);
      const model = await store.load(conn);
      const relations = relationsFor(model);
      let ids = null;
      if (args.table) {
        const wanted = String(args.table).toLowerCase();
        const t = model.tables.find((x) => x.id.toLowerCase() === wanted || x.name.toLowerCase() === wanted);
        if (!t) throw new Error(`No table “${args.table}”.`);
        ids = neighbourhood(model, relations, t.id, Math.max(0, Math.min(5, Math.round(Number(args.depth ?? 1)) || 0)));
      }
      if (args.show) {
        pending = { key: conn.key, table: args.table ?? null };
        api.openPage("erd");
        store.focus(conn.key, args.table ?? null);
      }
      const format = args.format ?? "mermaid-er";
      const text = format === "plantuml" ? toPlantUml(model, relations, ids) : format === "mermaid-class" ? toMermaidClass(model, relations, ids) : toMermaidEr(model, relations, ids);
      return text.length > AGENT_CHARS ? `${text.slice(0, AGENT_CHARS)}\n%% cut; narrow it with table and depth` : text;
    },
  });

  api.registerPage({ id: "erd", title: "Schema", icon: "network", mount: (el) => mountPage(el, api, store, () => pending, () => (pending = null)) });

  api.registerTool({
    id: "open",
    title: "Schema Diagram",
    description: "Show the database schema as a class diagram with tables, columns, keys and relations",
    palette: [{ title: "Schema Diagram: show database diagram" }],
    run: () => api.openPage("erd"),
  });
}

// Page --------------------------------------------------------------------------------------------

const SVG_NS = "http://www.w3.org/2000/svg";

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

function s(tag, attrs = {}, ...children) {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else el.setAttribute(key, String(value));
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

const ICONS = {
  logo: "M3 3h7v5H3zM14 3h7v5h-7zM8.5 16h7v5h-7zM6.5 8v3.5h11V8M12 11.5V16",
  plus: "M12 5v14M5 12h14",
  pencil: "M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z",
  trash: "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6",
  refresh: "M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5",
  search: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3",
  zoomIn: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3M11 8v6M8 11h6",
  zoomOut: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3M8 11h6",
  fit: "M3 8V3h5M16 3h5v5M21 16v5h-5M8 21H3v-5",
  layout: "M3 3h7v7H3zM14 3h7v7h-7zM14 14h7v7h-7zM3 14h7v7H3z",
  copy: "M9 9h11v11H9zM5 15H4V4h11v1",
  download: "M12 3v12M7 10l5 5 5-5M5 21h14",
  close: "M18 6 6 18M6 6l12 12",
  key: "M15.5 7.5a3.5 3.5 0 1 1-7 0 3.5 3.5 0 0 1 7 0zM12 11v10M12 15h3M12 18h2",
  link: "M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7",
  alert: "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
  check: "M20 6 9 17l-5-5",
  arrowLeft: "M19 12H5M12 19l-7-7 7-7",
  arrowRight: "M5 12h14M12 5l7 7-7 7",
  eye: "M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
  lock: "M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4",
};

function icon(name, cls = "sd-icon") {
  const el = document.createElementNS(SVG_NS, "svg");
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("class", cls);
  for (const [k, v] of Object.entries({ fill: "none", stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round", "stroke-linejoin": "round" })) el.setAttribute(k, v);
  el.append(s("path", { d: ICONS[name] }));
  return el;
}

const kindTile = (kind) => h("div", { class: `sd-kind sd-${kind}` }, kind === "mssql" ? "MS" : KINDS[kind]?.short.slice(0, 2) ?? "DB");

const CSS = `
.sd-root { height: 100%; display: flex; flex-direction: column; min-height: 0; container-type: inline-size; }
.sd-icon { width: 14px; height: 14px; flex-shrink: 0; }
.sd-head { display: flex; align-items: center; gap: 10px; padding: 12px 14px; border-bottom: 1px solid var(--color-border); flex-wrap: wrap; }
.sd-logo { width: 32px; height: 32px; border-radius: 9px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; color: white; background: linear-gradient(160deg, #8b7cf6, #5b45d6); box-shadow: inset 0 1px 0 rgb(255 255 255 / .25), 0 4px 14px rgb(91 69 214 / .25); }
.sd-logo .sd-icon { width: 17px; height: 17px; }
.sd-title { display: flex; flex-direction: column; gap: 1px; min-width: 0; margin-right: 6px; }
.sd-title b { font-size: 14px; font-weight: 600; color: var(--color-fg); }
.sd-title span { font-size: 11px; color: var(--color-fg-subtle); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.sd-grow { flex: 1; }
.sd-select { height: 28px; padding: 0 8px; border-radius: 7px; border: 1px solid var(--color-border); background: var(--color-panel); color: var(--color-fg); font: inherit; font-size: 12px; max-width: 240px; }
.sd-icon-btn { display: inline-flex; align-items: center; justify-content: center; width: 28px; height: 28px; flex-shrink: 0; border-radius: 7px; border: 0; background: transparent; color: var(--color-fg-subtle); cursor: pointer; }
.sd-icon-btn:hover { background: var(--color-active); color: var(--color-fg); }
.sd-icon-btn.sd-bad:hover { color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 12%, transparent); }
.sd-icon-btn:disabled { opacity: .4; pointer-events: none; }
.sd-tools { display: flex; align-items: center; gap: 8px; padding: 8px 14px; border-bottom: 1px solid var(--color-border); flex-wrap: wrap; background: var(--color-panel); }
.sd-search { display: flex; align-items: center; gap: 6px; height: 28px; padding: 0 8px; border-radius: 7px; border: 1px solid var(--color-border); background: var(--color-bg, var(--color-panel)); width: 200px; }
.sd-search .sd-icon { width: 12px; height: 12px; color: var(--color-fg-subtle); }
.sd-search input { flex: 1; min-width: 0; border: 0; outline: none; background: transparent; color: var(--color-fg); font: inherit; font-size: 12px; }
.sd-seg { display: inline-flex; padding: 2px; border-radius: 8px; background: var(--color-active); gap: 2px; }
.sd-seg button { height: 24px; padding: 0 9px; border-radius: 6px; border: 0; background: transparent; color: var(--color-fg-muted); font: inherit; font-size: 11px; font-weight: 500; cursor: pointer; white-space: nowrap; }
.sd-seg button:hover { color: var(--color-fg); }
.sd-seg button.sd-on { background: var(--color-panel-raised, var(--color-panel)); color: var(--color-fg); box-shadow: 0 1px 2px rgb(0 0 0 / .12); }
.sd-check { display: inline-flex; align-items: center; gap: 6px; font-size: 11px; color: var(--color-fg-muted); white-space: nowrap; cursor: pointer; }
.sd-stats { font-size: 11px; color: var(--color-fg-subtle); white-space: nowrap; font-variant-numeric: tabular-nums; }
.sd-stats b { color: var(--color-fg-muted); font-weight: 600; }
.sd-body { flex: 1; min-height: 0; display: flex; position: relative; }
.sd-canvas { flex: 1; min-width: 0; position: relative; overflow: hidden; cursor: grab; background-color: var(--color-bg, var(--color-panel)); background-image: radial-gradient(circle, color-mix(in srgb, var(--color-fg) 9%, transparent) 1px, transparent 1px); background-size: 22px 22px; }
.sd-canvas.sd-panning { cursor: grabbing; }
.sd-canvas > svg { position: absolute; inset: 0; width: 100%; height: 100%; user-select: none; }
.sd-overlay { position: absolute; display: flex; gap: 4px; align-items: center; padding: 4px; border-radius: 10px; background: var(--color-panel-raised, var(--color-panel)); border: 1px solid var(--color-border); box-shadow: 0 4px 16px rgb(0 0 0 / .12); }
.sd-zoom { right: 12px; bottom: 12px; }
.sd-zoom span { min-width: 42px; text-align: center; font-size: 11px; color: var(--color-fg-muted); font-variant-numeric: tabular-nums; }
.sd-legend { left: 12px; bottom: 12px; padding: 8px 10px; gap: 14px; font-size: 11px; color: var(--color-fg-muted); }
.sd-legend > div { display: flex; align-items: center; gap: 6px; }
.sd-legend svg { overflow: visible; }
@container (max-width: 760px) { .sd-legend { display: none; } }
.sd-note { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; padding: 24px; text-align: center; color: var(--color-fg-subtle); font-size: 12px; }
.sd-note.sd-err pre { max-width: 640px; white-space: pre-wrap; text-align: left; font-family: var(--font-mono); font-size: 11px; color: var(--color-danger); user-select: text; margin: 0; padding: 12px 14px; border-radius: 8px; background: color-mix(in srgb, var(--color-danger) 8%, transparent); border: 1px solid color-mix(in srgb, var(--color-danger) 22%, transparent); }
.sd-spinner { width: 22px; height: 22px; border-radius: 50%; border: 2px solid var(--color-border); border-top-color: var(--color-accent); animation: sd-spin .8s linear infinite; }
@keyframes sd-spin { to { transform: rotate(360deg); } }

/* Diagram */
.sd-box rect.sd-card { fill: var(--color-panel-raised, var(--color-panel)); stroke: var(--color-border-strong, var(--color-border)); stroke-width: 1; }
.sd-box rect.sd-band { fill: color-mix(in srgb, var(--sd-tone) 16%, var(--color-panel-raised, var(--color-panel))); }
.sd-box line.sd-rule { stroke: var(--color-border); }
.sd-box { --sd-tone: #6d5ce8; cursor: move; transition: opacity .15s; }
.sd-box.sd-view { --sd-tone: #2b9fb3; }
.sd-box.sd-junction { --sd-tone: #d08a2e; }
.sd-box.sd-external { --sd-tone: #8a8f98; }
.sd-box.sd-external rect.sd-card { stroke-dasharray: 4 3; }
.sd-box.sd-view rect.sd-card { stroke-dasharray: 5 2; }
.sd-box text { font-size: 11.5px; fill: var(--color-fg-muted); }
.sd-box text.sd-name { font-size: 12.5px; font-weight: 600; fill: var(--color-fg); }
.sd-box text.sd-stereo { font-size: 10px; fill: var(--color-fg-subtle); font-style: italic; }
.sd-box text.sd-count { font-size: 10px; fill: var(--color-fg-subtle); }
.sd-box text.sd-col { font-family: var(--font-mono); fill: var(--color-fg); }
.sd-box text.sd-col.sd-nullable { fill: var(--color-fg-muted); }
.sd-box text.sd-type { font-family: var(--font-mono); fill: var(--color-fg-subtle); }
.sd-box text.sd-badge { font-size: 8.5px; font-weight: 700; letter-spacing: .02em; }
.sd-box text.sd-pk { fill: #d9a520; }
.sd-box text.sd-fk { fill: var(--sd-tone); }
.sd-box text.sd-uk { fill: #2bb39e; }
.sd-box rect.sd-hit { fill: transparent; }
.sd-box rect.sd-row-hl { fill: color-mix(in srgb, var(--color-accent) 14%, transparent); }
.sd-box.sd-selected rect.sd-card { stroke: var(--color-accent); stroke-width: 2; }
.sd-box.sd-match rect.sd-card { stroke: var(--color-warning, #e3973a); stroke-width: 2; }
.sd-dim .sd-box:not(.sd-lit) { opacity: .28; }
.sd-dim .sd-edge:not(.sd-lit) { opacity: .12; }
.sd-edge { transition: opacity .15s; cursor: pointer; }
.sd-edge path.sd-line { fill: none; stroke: color-mix(in srgb, var(--color-fg) 42%, transparent); stroke-width: 1.4; }
.sd-edge path.sd-wide { fill: none; stroke: transparent; stroke-width: 12; }
.sd-edge.sd-inferred path.sd-line { stroke-dasharray: 5 4; stroke: color-mix(in srgb, var(--color-warning, #e3973a) 70%, transparent); }
.sd-edge .sd-end { fill: none; stroke: color-mix(in srgb, var(--color-fg) 55%, transparent); stroke-width: 1.4; }
.sd-edge .sd-end-dot { fill: var(--color-panel-raised, var(--color-panel)); stroke: color-mix(in srgb, var(--color-fg) 55%, transparent); stroke-width: 1.3; }
.sd-edge text { font-size: 10.5px; font-family: var(--font-mono); fill: var(--color-fg-muted); paint-order: stroke; stroke: var(--color-bg, var(--color-panel)); stroke-width: 3px; }
.sd-edge:hover path.sd-line, .sd-edge.sd-lit path.sd-line { stroke: var(--color-accent); stroke-width: 2; }
.sd-edge:hover .sd-end, .sd-edge.sd-lit .sd-end, .sd-edge:hover .sd-end-dot, .sd-edge.sd-lit .sd-end-dot { stroke: var(--color-accent); }

/* Side panel */
.sd-side { width: 320px; flex-shrink: 0; border-left: 1px solid var(--color-border); background: var(--color-panel); overflow: auto; display: flex; flex-direction: column; }
@container (max-width: 760px) { .sd-side { position: absolute; right: 0; top: 0; bottom: 0; z-index: 2; max-width: 85%; box-shadow: -8px 0 24px rgb(0 0 0 / .15); } .sd-side.sd-overview { display: none !important; } }
.sd-side-head { display: flex; align-items: flex-start; gap: 8px; padding: 14px 12px 10px 14px; border-bottom: 1px solid var(--color-border); }
.sd-side-head > div { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.sd-side-head b { font-size: 13px; font-family: var(--font-mono); color: var(--color-fg); word-break: break-all; }
.sd-side-head span { font-size: 11px; color: var(--color-fg-subtle); }
.sd-sec { padding: 10px 14px; display: flex; flex-direction: column; gap: 4px; }
.sd-sec + .sd-sec { border-top: 1px solid var(--color-border); }
.sd-sec h4 { margin: 0 0 4px; font-size: 10.5px; font-weight: 600; text-transform: uppercase; letter-spacing: .05em; color: var(--color-fg-subtle); }
.sd-colrow { display: grid; grid-template-columns: 26px minmax(0, 1fr) auto; gap: 6px; align-items: baseline; font-size: 11.5px; padding: 2px 0; }
.sd-colrow .sd-k { font-size: 9px; font-weight: 700; }
.sd-colrow .sd-k.sd-pk { color: #d9a520; } .sd-colrow .sd-k.sd-fk { color: #6d5ce8; } .sd-colrow .sd-k.sd-uk { color: #2bb39e; }
.sd-colrow code { font-family: var(--font-mono); color: var(--color-fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sd-colrow small { font-family: var(--font-mono); font-size: 10.5px; color: var(--color-fg-subtle); white-space: nowrap; }
.sd-colrow .sd-def { grid-column: 2 / 4; font-size: 10.5px; color: var(--color-fg-subtle); font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; margin-top: -2px; }
.sd-rel { display: flex; align-items: center; gap: 6px; padding: 5px 6px; margin: 0 -6px; border-radius: 6px; font-size: 11.5px; color: var(--color-fg-muted); cursor: pointer; border: 0; background: transparent; text-align: left; font: inherit; }
.sd-rel:hover { background: var(--color-hover); color: var(--color-fg); }
.sd-rel code { font-family: var(--font-mono); color: var(--color-fg); }
.sd-rel .sd-card-tag { margin-left: auto; font-family: var(--font-mono); font-size: 10px; color: var(--color-fg-subtle); white-space: nowrap; }
.sd-link { display: inline; padding: 0; border: 0; background: none; font: inherit; font-family: var(--font-mono); font-size: 11px; color: var(--color-fg); cursor: pointer; text-decoration: underline; text-decoration-color: var(--color-border-strong); text-underline-offset: 2px; }
.sd-link:hover { color: var(--color-accent); }
.sd-tag { display: inline-flex; align-items: center; height: 17px; padding: 0 6px; border-radius: 4px; font-size: 10px; font-weight: 500; background: var(--color-active); color: var(--color-fg-muted); white-space: nowrap; }
.sd-tag.sd-warn { color: var(--color-warning, #e3973a); background: color-mix(in srgb, var(--color-warning, #e3973a) 13%, transparent); }
.sd-ix { font-size: 11px; font-family: var(--font-mono); color: var(--color-fg-muted); display: flex; gap: 6px; align-items: baseline; }
.sd-ix span:first-child { color: var(--color-fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sd-insight { display: flex; gap: 8px; align-items: flex-start; font-size: 11.5px; line-height: 1.45; color: var(--color-fg-muted); }
.sd-insight .sd-icon { margin-top: 1px; color: var(--color-warning, #e3973a); }
.sd-insight code { font-family: var(--font-mono); font-size: 11px; color: var(--color-fg); }

/* Empty + dialog (like DB Explorer's) */
.sd-empty { margin: auto; display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 40px 20px; text-align: center; }
.sd-empty .sd-logo { width: 44px; height: 44px; margin-bottom: 8px; }
.sd-empty b { font-size: 14px; color: var(--color-fg); }
.sd-starters { display: flex; gap: 8px; flex-wrap: wrap; justify-content: center; margin-top: 14px; }
.sd-starter { display: flex; align-items: center; gap: 10px; padding: 8px 12px 8px 8px; border-radius: 9px; border: 1px solid var(--color-border); background: var(--color-panel); font: inherit; font-size: 12px; font-weight: 500; color: var(--color-fg); cursor: pointer; }
.sd-starter:hover { border-color: var(--color-border-strong); background: var(--color-hover); }
.sd-kind { --sd-k: var(--color-fg-subtle); width: 26px; height: 26px; border-radius: 7px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; font-size: 10px; font-weight: 700; color: var(--sd-k); background: color-mix(in srgb, var(--sd-k) 13%, transparent); border: 1px solid color-mix(in srgb, var(--sd-k) 22%, transparent); }
.sd-kind.sd-postgres { --sd-k: #4f8fdc; } .sd-kind.sd-mysql { --sd-k: #e3973a; } .sd-kind.sd-sqlite { --sd-k: #2bb39e; } .sd-kind.sd-mssql { --sd-k: #c94f5d; }
.sd-dialog { width: min(500px, 92vw); padding: 0; gap: 0; }
.sd-dialog-head { display: flex; align-items: center; gap: 12px; padding: 18px 20px 14px; }
.sd-dialog-body { display: flex; flex-direction: column; gap: 14px; padding: 4px 20px 18px; }
.sd-dialog-foot { display: flex; align-items: center; gap: 8px; padding: 12px 20px; border-top: 1px solid var(--color-border); background: var(--color-panel); }
.sd-field { display: flex; flex-direction: column; gap: 6px; font-size: 12px; font-weight: 500; color: var(--color-fg-muted); }
.sd-field small { font-size: 11px; font-weight: 400; color: var(--color-fg-subtle); }
.sd-input { display: flex; align-items: center; gap: 6px; height: 32px; padding: 0 10px; border-radius: 8px; border: 1px solid var(--color-border-strong); background: var(--color-panel); }
.sd-input:focus-within { border-color: var(--color-accent); box-shadow: 0 0 0 3px var(--color-accent-soft); }
.sd-input input { flex: 1; min-width: 0; height: 100%; border: 0; outline: none; background: transparent; color: var(--color-fg); font: inherit; font-size: 12px; }
.sd-input input.sd-mono { font-family: var(--font-mono); }
.sd-kinds { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 6px; }
.sd-kind-opt { display: flex; align-items: center; gap: 8px; padding: 8px 10px; border-radius: 9px; border: 1px solid var(--color-border); background: var(--color-panel); font: inherit; font-size: 12px; font-weight: 500; color: var(--color-fg-muted); cursor: pointer; }
.sd-kind-opt.sd-on { border-color: var(--color-accent); background: color-mix(in srgb, var(--color-accent) 8%, var(--color-panel)); color: var(--color-fg); }
.sd-alert { display: flex; gap: 8px; align-items: flex-start; padding: 9px 12px; border-radius: 8px; font-size: 12px; line-height: 1.5; color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 9%, transparent); border: 1px solid color-mix(in srgb, var(--color-danger) 25%, transparent); white-space: pre-wrap; user-select: text; }
.sd-alert.sd-ok { color: var(--color-success); background: color-mix(in srgb, var(--color-success) 9%, transparent); border-color: color-mix(in srgb, var(--color-success) 25%, transparent); }
`;

/** Text widths with the page's real fonts. */
function measurer() {
  const ctx = document.createElement("canvas").getContext("2d");
  const css = getComputedStyle(document.documentElement);
  const mono = css.getPropertyValue("--font-mono").trim() || "ui-monospace, monospace";
  const sans = getComputedStyle(document.body).fontFamily || "system-ui";
  const cache = new Map();
  return (text, font) => {
    const k = `${font}\u0000${text}`;
    if (!cache.has(k)) {
      ctx.font = font === "mono" ? `11.5px ${mono}` : `600 12.5px ${sans}`;
      cache.set(k, ctx.measureText(String(text)).width);
    }
    return cache.get(k);
  };
}

/** Cuts `text` with an ellipsis to fit `width`. */
function fit(text, width, measure, font) {
  if (measure(text, font) <= width) return text;
  let t = String(text);
  while (t.length > 1 && measure(`${t}…`, font) > width) t = t.slice(0, -1);
  return `${t}…`;
}

function mountPage(root, api, store, takePending, clearPending) {
  let conns = [];
  let selectedKey = null;
  let selectedTable = null;
  let schemaFilter = "";
  let search = "";
  let p = prefs();
  let view = { x: 40, y: 40, k: 1 };
  let model = null;
  let relations = [];
  let sizes = new Map();
  let positions = new Map();
  let manual = {};
  const measure = measurer();

  const style = h("style", {}, CSS);
  const page = h("div", { class: "sd-root" });
  root.append(style, page);

  const current = () => conns.find((c) => c.key === selectedKey) ?? null;
  const setPrefs = (patch) => {
    p = { ...p, ...patch };
    saveJson(PREFS, p);
  };

  // Static parts --------------------------------------------------------------------------------
  const connSelect = h("select", { class: "sd-select", title: "Database", onchange: (e) => select(e.target.value) });
  const subtitle = h("span", {});
  const reloadBtn = h("button", { class: "sd-icon-btn", title: "Read the schema again", onclick: () => current() && load(current(), true) }, icon("refresh"));
  const editBtn = h("button", { class: "sd-icon-btn", title: "Edit connection", onclick: () => current() && openEditor(current()) }, icon("pencil"));
  const removeBtn = h("button", { class: "sd-icon-btn sd-bad", title: "Remove connection", onclick: () => removeConn() }, icon("trash"));
  const head = h(
    "div",
    { class: "sd-head" },
    h("div", { class: "sd-logo" }, icon("logo")),
    h("div", { class: "sd-title" }, h("b", {}, "Schema Diagram"), subtitle),
    connSelect,
    reloadBtn,
    editBtn,
    removeBtn,
    h("span", { class: "sd-grow" }),
    h("button", { class: "wf-button", onclick: () => openEditor(null) }, icon("plus"), "Add database"),
  );

  const searchInput = h("input", {
    type: "search",
    placeholder: "Find a table or column",
    spellcheck: "false",
    oninput: (e) => {
      search = e.target.value.trim().toLowerCase();
      draw();
    },
    onkeydown: (e) => {
      if (e.key !== "Enter") return;
      const hit = matches()[0];
      if (hit) focusTable(hit, true);
    },
  });
  const schemaSelect = h("select", {
    class: "sd-select",
    title: "Schema",
    onchange: (e) => {
      schemaFilter = e.target.value;
      selectedTable = null;
      relayout(true);
    },
  });
  const seg = (options, value, onpick) =>
    h(
      "div",
      { class: "sd-seg", role: "radiogroup" },
      options.map(([v, label, title]) => h("button", { class: v === value ? "sd-on" : "", title, role: "radio", "aria-checked": String(v === value), onclick: () => onpick(v) }, label)),
    );
  const colSeg = h("span");
  const notationSeg = h("span");
  const inferredBox = h("input", {
    type: "checkbox",
    class: "wf-switch",
    checked: p.inferred,
    onchange: (e) => {
      setPrefs({ inferred: e.target.checked });
      rebuildRelations();
      relayout(false);
    },
  });
  const stats_ = h("span", { class: "sd-stats" });
  const exportSelect = h(
    "select",
    {
      class: "sd-select",
      title: "Export the diagram",
      onchange: (e) => {
        const v = e.target.value;
        e.target.value = "";
        void exportAs(v);
      },
    },
    h("option", { value: "" }, "Export…"),
    h("option", { value: "mermaid-er" }, "Copy Mermaid ER diagram"),
    h("option", { value: "mermaid-class" }, "Copy Mermaid class diagram"),
    h("option", { value: "plantuml" }, "Copy PlantUML"),
    h("option", { value: "svg" }, "Save as SVG"),
  );
  const tools = h(
    "div",
    { class: "sd-tools" },
    h("label", { class: "sd-search" }, icon("search"), searchInput),
    schemaSelect,
    colSeg,
    notationSeg,
    h("label", { class: "sd-check", title: "Draw relations the column names suggest but no foreign key declares, dashed" }, inferredBox, "Inferred relations"),
    h("span", { class: "sd-grow" }),
    stats_,
    exportSelect,
  );

  const svg = s("svg", { "aria-label": "Schema diagram" });
  const world = s("g", {});
  const edgeLayer = s("g", {});
  const boxLayer = s("g", {});
  world.append(edgeLayer, boxLayer);
  svg.append(world);
  const zoomLabel = h("span", {});
  const zoomBar = h(
    "div",
    { class: "sd-overlay sd-zoom" },
    h("button", { class: "sd-icon-btn", title: "Zoom out", onclick: () => zoomBy(1 / 1.25) }, icon("zoomOut")),
    zoomLabel,
    h("button", { class: "sd-icon-btn", title: "Zoom in", onclick: () => zoomBy(1.25) }, icon("zoomIn")),
    h("button", { class: "sd-icon-btn", title: "Fit to screen", onclick: () => fitView() }, icon("fit")),
    h("button", { class: "sd-icon-btn", title: "Arrange automatically (forget moved tables)", onclick: () => resetLayout() }, icon("layout")),
  );
  const legend = h("div", { class: "sd-overlay sd-legend" });
  const note = h("div", { class: "sd-note" });
  const canvas = h("div", { class: "sd-canvas" }, svg, legend, zoomBar, note);
  const side = h("aside", { class: "sd-side", style: "display:none" });
  const body = h("div", { class: "sd-body" }, canvas, side);
  const shell = h("div", { style: "display:contents" }, head, tools, body);

  // Data ----------------------------------------------------------------------------------------
  async function refresh() {
    try {
      conns = await store.connections();
    } catch (err) {
      conns = [];
      api.toast(String(err?.message ?? err), "error");
    }
    const want = takePending();
    if (want && conns.some((c) => c.key === want.key)) selectedKey = want.key;
    if (!current()) selectedKey = loadJson(`${PREFS}.last`, null);
    if (!current()) selectedKey = conns[0]?.key ?? null;
    render();
    if (current()) await load(current(), false, want?.table ?? null);
    clearPending();
  }

  async function load(conn, fresh, focusName = null) {
    try {
      const promise = store.load(conn, { fresh });
      showModel();
      await promise;
    } catch {
      // The store keeps the error; showModel shows it.
    }
    if (conn.key !== selectedKey) return;
    showModel(true);
    if (focusName) focusByName(focusName);
  }

  function select(key) {
    if (key === selectedKey) return;
    selectedKey = key;
    selectedTable = null;
    schemaFilter = "";
    saveJson(`${PREFS}.last`, key);
    render();
    if (current()) void load(current(), false);
  }

  async function removeConn() {
    const c = current();
    if (!c) return;
    if (!(await api.confirm(`Remove ${c.meta.name || c.key}? Its credentials are deleted from your keychain. The database itself isn't touched.`, { title: "Remove database", okLabel: "Remove" }))) return;
    try {
      await api.vault.delete(c.key);
      store.forget(c.key);
      localStorage.removeItem(LAYOUTS + c.key);
    } catch (err) {
      api.toast(String(err?.message ?? err), "error");
    }
    selectedKey = null;
    await refresh();
  }

  function rebuildRelations() {
    relations = model ? [...model.relations, ...(p.inferred ? inferRelations(model) : [])] : [];
  }

  const visibleTables = () => (model ? model.tables.filter((t) => !schemaFilter || t.schema === schemaFilter || (t.kind === "external" && relations.some((r) => r.to === t.id && model.tables.find((x) => x.id === r.from)?.schema === schemaFilter))) : []);
  const tableById = (id) => model?.tables.find((t) => t.id === id) ?? null;

  /** Puts the current connection's model (or its loading state / error) on the canvas. */
  function showModel(fitAfter = false) {
    const conn = current();
    const st = conn ? store.schema(conn.key) : null;
    reloadBtn.disabled = !conn || st?.state === "loading";
    if (!st || st.state === "loading") {
      model = null;
      note.replaceChildren(h("div", { class: "sd-spinner" }), `Reading the schema of ${conn?.meta.name ?? "the database"}…`);
      note.className = "sd-note";
      note.style.display = "";
      clearDiagram();
      return;
    }
    if (st.state === "error") {
      model = null;
      note.replaceChildren(h("b", { style: "color:var(--color-fg)" }, "Couldn't read the schema"), h("pre", {}, st.error), h("button", { class: "wf-button", onclick: () => load(conn, true) }, icon("refresh"), "Try again"));
      note.className = "sd-note sd-err";
      note.style.display = "";
      clearDiagram();
      return;
    }
    const fresh = model !== st.model;
    model = st.model;
    if (model.tables.length === 0) {
      note.replaceChildren(h("b", { style: "color:var(--color-fg)" }, "No tables"), (conn.meta.kind === "mysql" || conn.meta.kind === "mssql") && !conn.meta.database ? `The URL names no database: add one after the port, like ${KINDS[conn.meta.kind].placeholder}.` : "This database has no tables or views yet.");
      note.className = "sd-note";
      note.style.display = "";
      clearDiagram();
      renderTools();
      return;
    }
    note.style.display = "none";
    if (fresh) {
      manual = loadJson(LAYOUTS + conn.key, {});
      rebuildRelations();
      if (schemaFilter && !model.schemas.includes(schemaFilter)) schemaFilter = "";
      // Many schemas: start on the biggest that isn't empty, usually "public".
      if (!schemaFilter && model.schemas.length > 1 && model.tables.length > 60) schemaFilter = ["public", "dbo"].find((x) => model.schemas.includes(x)) ?? model.schemas[0];
      if (selectedTable && !tableById(selectedTable)) selectedTable = null;
    }
    renderTools();
    relayout(fresh || fitAfter);
  }

  function clearDiagram() {
    edgeLayer.replaceChildren();
    boxLayer.replaceChildren();
    side.style.display = "none";
    stats_.replaceChildren();
  }

  // Layout + drawing -------------------------------------------------------------------------------
  function relayout(fitAfter) {
    if (!model) return;
    const tables = visibleTables();
    sizes = new Map(tables.map((t) => [t.id, measureTable(t, p.columns, measure)]));
    const auto = layout(tables, relations, sizes);
    positions = new Map(tables.map((t) => [t.id, manual[t.id] ? { ...manual[t.id] } : auto.get(t.id)]));
    draw();
    if (fitAfter) requestAnimationFrame(() => fitView());
  }

  function resetLayout() {
    const conn = current();
    if (!conn) return;
    manual = {};
    localStorage.removeItem(LAYOUTS + conn.key);
    relayout(true);
  }

  function matches() {
    if (!search || !model) return [];
    return visibleTables()
      .filter((t) => t.name.toLowerCase().includes(search) || t.columns.some((c) => c.name.toLowerCase().includes(search)))
      .sort((a, b) => (a.name.toLowerCase() === search ? -1 : 0) - (b.name.toLowerCase() === search ? -1 : 0) || (a.name.toLowerCase().includes(search) ? 0 : 1) - (b.name.toLowerCase().includes(search) ? 0 : 1))
      .map((t) => t.id);
  }

  /** The y of a column's row inside its box (or the header's middle when it isn't shown). */
  function rowY(table, column) {
    const cols = visibleColumns(table, p.columns);
    const i = cols.findIndex((c) => c.name === column);
    const top = METRICS.header + (stereotype(table) ? METRICS.stereo : 0);
    if (i < 0) return (METRICS.header + (stereotype(table) ? METRICS.stereo : 0)) / 2;
    return top + i * METRICS.row + METRICS.row / 2 + 2;
  }

  /** Endpoints and path of a relation: from the referencing column to the referenced one. */
  function edgeGeometry(r) {
    const a = positions.get(r.from);
    const b = positions.get(r.to);
    const sa = sizes.get(r.from);
    const sb = sizes.get(r.to);
    const ta = tableById(r.from);
    const tb = tableById(r.to);
    const y1 = a.y + rowY(ta, r.fromColumns[0]);
    const y2 = b.y + rowY(tb, r.toColumns[0]);
    let x1, x2, d1, d2;
    if (r.from === r.to) {
      x1 = x2 = a.x + sa.w;
      d1 = d2 = 1;
      const bulge = 46;
      const yy2 = y1 === y2 ? y2 + 14 : y2;
      return { x1, y1, x2, y2: yy2, d1, d2, path: `M${x1},${y1} C${x1 + bulge},${y1} ${x2 + bulge},${yy2} ${x2},${yy2}` };
    }
    if (a.x + sa.w + 24 <= b.x) [x1, d1, x2, d2] = [a.x + sa.w, 1, b.x, -1];
    else if (b.x + sb.w + 24 <= a.x) [x1, d1, x2, d2] = [a.x, -1, b.x + sb.w, 1];
    else {
      // Stacked: leave and arrive on the same side, whichever is nearer.
      const right = Math.max(a.x + sa.w, b.x + sb.w);
      const left = Math.min(a.x, b.x);
      const useRight = Math.abs(a.x + sa.w - (b.x + sb.w)) <= Math.abs(a.x - b.x) || right - a.x < a.x + sa.w - left;
      [x1, d1, x2, d2] = useRight ? [a.x + sa.w, 1, b.x + sb.w, 1] : [a.x, -1, b.x, -1];
      const reach = 36 + Math.abs(x1 - x2);
      return { x1, y1, x2, y2, d1, d2, path: `M${x1},${y1} C${x1 + d1 * reach},${y1} ${x2 + d2 * reach},${y2} ${x2},${y2}` };
    }
    const k = Math.max(36, Math.abs(x2 - x1) / 2);
    return { x1, y1, x2, y2, d1, d2, path: `M${x1},${y1} C${x1 + d1 * k},${y1} ${x2 + d2 * k},${y2} ${x2},${y2}` };
  }

  /** Crow's-foot end marks: `many` (crow), `one` (bar), `opt` (circle) at (x, y) facing out along d. */
  function crowEnd(x, y, d, { many, optional }) {
    const g = s("g", {});
    if (many) g.append(s("path", { class: "sd-end", d: `M${x + d * 12},${y} L${x},${y - 6} M${x + d * 12},${y} L${x},${y} M${x + d * 12},${y} L${x},${y + 6}` }));
    else g.append(s("path", { class: "sd-end", d: `M${x + d * 8},${y - 6} L${x + d * 8},${y + 6}` }));
    if (optional) g.append(s("circle", { class: "sd-end-dot", cx: x + d * 21, cy: y, r: 4 }));
    else g.append(s("path", { class: "sd-end", d: `M${x + d * 16},${y - 6} L${x + d * 16},${y + 6}` }));
    return g;
  }

  function umlEnd(x, y, d, label, arrow) {
    const g = s("g", {});
    if (arrow) g.append(s("path", { class: "sd-end", d: `M${x + d * 10},${y - 5} L${x},${y} L${x + d * 10},${y + 5}` }));
    g.append(s("text", { x: x + d * 8, y: y - 6, "text-anchor": d > 0 ? "start" : "end" }, label));
    return g;
  }

  function edgeTitle(r) {
    const from = tableById(r.from);
    const to = tableById(r.to);
    return `${from.name}(${r.fromColumns.join(", ")}) → ${to.name}(${r.toColumns.join(", ")})\n${r.one ? "one-to-one" : "many-to-one"}${r.optional ? ", optional" : ""}${r.onDelete ? ` · ON DELETE ${r.onDelete}` : ""}${r.onUpdate ? ` · ON UPDATE ${r.onUpdate}` : ""}${r.inferred ? "\nInferred from the column name: no foreign key declares it." : `\n${r.name}`}`;
  }

  function lit() {
    const set = new Set();
    if (!selectedTable) return { tables: set, edges: new Set() };
    set.add(selectedTable);
    const edges = new Set();
    for (const r of relations) {
      if (r.from === selectedTable || r.to === selectedTable) {
        edges.add(r.id);
        set.add(r.from);
        set.add(r.to);
      }
    }
    return { tables: set, edges };
  }

  function drawEdges() {
    const shown = new Set(positions.keys());
    const { edges } = lit();
    edgeLayer.replaceChildren(
      ...relations
        .filter((r) => shown.has(r.from) && shown.has(r.to))
        .map((r) => {
          const g = edgeGeometry(r);
          const el = s(
            "g",
            {
              class: `sd-edge${r.inferred ? " sd-inferred" : ""}${edges.has(r.id) ? " sd-lit" : ""}`,
              onclick: (e) => {
                e.stopPropagation();
                selectTable(selectedTable === r.from ? r.to : r.from);
              },
            },
            s("title", {}, edgeTitle(r)),
            s("path", { class: "sd-wide", d: g.path }),
            s("path", { class: "sd-line", d: g.path }),
          );
          if (p.notation === "uml") {
            el.append(umlEnd(g.x1, g.y1, g.d1, r.one ? "0..1" : "*", false), umlEnd(g.x2, g.y2, g.d2, r.optional ? "0..1" : "1", true));
          } else {
            el.append(crowEnd(g.x1, g.y1, g.d1, { many: !r.one, optional: true }), crowEnd(g.x2, g.y2, g.d2, { many: false, optional: r.optional }));
          }
          return el;
        }),
    );
  }

  function drawBox(t) {
    const pos = positions.get(t.id);
    const { w, h: height } = sizes.get(t.id);
    const st = stereotype(t);
    const top = METRICS.header + (st ? METRICS.stereo : 0);
    const found = search && (t.name.toLowerCase().includes(search) || t.columns.some((c) => c.name.toLowerCase().includes(search)));
    const g = s("g", {
      class: `sd-box${t.kind === "view" || t.kind === "materialized view" ? " sd-view" : ""}${t.junction ? " sd-junction" : ""}${t.kind === "external" ? " sd-external" : ""}${selectedTable === t.id ? " sd-selected" : ""}${found ? " sd-match" : ""}`,
      transform: `translate(${pos.x},${pos.y})`,
      "data-table": t.id,
    });
    g.append(
      s("rect", { class: "sd-card", width: w, height, rx: 8 }),
      s("path", { class: "sd-band", d: `M0,8 a8,8 0 0 1 8,-8 h${w - 16} a8,8 0 0 1 8,8 v${top - 8} h-${w} z`, style: "fill:color-mix(in srgb, var(--sd-tone) 16%, var(--color-panel-raised, var(--color-panel)))" }),
      s("rect", { width: 3, height: top - 10, x: 0, y: 5, rx: 1.5, style: "fill:var(--sd-tone)" }),
    );
    if (st) g.append(s("text", { class: "sd-stereo", x: 12, y: 15 }, st));
    const count = `${t.columns.length}`;
    g.append(s("text", { class: "sd-name", x: 12, y: st ? 33 : 20 }, fit(t.name, w - 44, measure, "sans")));
    g.append(s("text", { class: "sd-count", x: w - 10, y: st ? 33 : 20, "text-anchor": "end" }, t.rows !== null && t.kind !== "view" ? `${compact(t.rows)} rows` : count === "0" ? "" : `${count} cols`));
    const cols = visibleColumns(t, p.columns);
    if (cols.length) g.append(s("line", { class: "sd-rule", x1: 0, x2: w, y1: top, y2: top }));
    const hl = selectedTable && selectedTable !== t.id ? highlightedColumns(t.id) : new Set();
    cols.forEach((c, i) => {
      const y = top + i * METRICS.row + 4;
      if (hl.has(c.name)) g.append(s("rect", { class: "sd-row-hl", x: 1, y, width: w - 2, height: METRICS.row }));
      const badge = c.pk ? ["PK", "sd-pk"] : c.fk ? ["FK", "sd-fk"] : c.unique ? ["UK", "sd-uk"] : null;
      if (badge) g.append(s("text", { class: `sd-badge ${badge[1]}`, x: 10, y: y + 14 }, badge[0]));
      if (c.pk && c.fk) g.append(s("text", { class: "sd-badge sd-fk", x: 10, y: y + 6, style: "font-size:7px" }, "FK"));
      const typeText = `${shortType(c.type)}${c.nullable ? "?" : ""}`;
      const typeW = Math.min(measure(typeText, "mono"), w * 0.45);
      g.append(s("text", { class: `sd-col${c.nullable ? " sd-nullable" : ""}`, x: 30, y: y + 14 }, s("title", {}, `${c.name} ${c.type}${c.nullable ? "" : " NOT NULL"}${c.default ? ` DEFAULT ${c.default}` : ""}${c.comment ? `\n${c.comment}` : ""}`), fit(c.name, w - 30 - typeW - 20, measure, "mono")));
      g.append(s("text", { class: "sd-type", x: w - 10, y: y + 14, "text-anchor": "end" }, fit(typeText, w * 0.45, measure, "mono")));
    });
    if (t.comment) g.append(s("title", {}, t.comment));
    g.addEventListener("pointerdown", (e) => startDrag(e, t.id));
    return g;
  }

  /** Columns of `id` on the selected table's relations, to light up. */
  function highlightedColumns(id) {
    const out = new Set();
    for (const r of relations) {
      if (r.from === id && r.to === selectedTable) r.fromColumns.forEach((c) => out.add(c));
      if (r.to === id && r.from === selectedTable) r.toColumns.forEach((c) => out.add(c));
    }
    return out;
  }

  function draw() {
    if (!model) return;
    const tables = visibleTables().filter((t) => positions.has(t.id));
    const { tables: litTables } = lit();
    world.setAttribute("class", selectedTable ? "sd-dim" : "");
    boxLayer.replaceChildren(
      ...tables.map((t) => {
        const g = drawBox(t);
        if (litTables.has(t.id)) g.classList.add("sd-lit");
        return g;
      }),
    );
    drawEdges();
    for (const g of edgeLayer.children) if (g.classList.contains("sd-lit")) g.parentNode.append(g);
    applyView();
    renderStats();
    renderSide();
    renderLegend();
  }

  // Pan, zoom, drag --------------------------------------------------------------------------------
  function applyView() {
    world.setAttribute("transform", `translate(${view.x},${view.y}) scale(${view.k})`);
    zoomLabel.textContent = `${Math.round(view.k * 100)}%`;
  }

  function zoomAt(factor, cx, cy) {
    const k = Math.min(2.5, Math.max(0.08, view.k * factor));
    view = { k, x: cx - ((cx - view.x) * k) / view.k, y: cy - ((cy - view.y) * k) / view.k };
    applyView();
  }

  function zoomBy(factor) {
    const r = canvas.getBoundingClientRect();
    zoomAt(factor, r.width / 2, r.height / 2);
  }

  function bounds(ids = [...positions.keys()]) {
    let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const id of ids) {
      const pos = positions.get(id);
      const sz = sizes.get(id);
      if (!pos || !sz) continue;
      x0 = Math.min(x0, pos.x);
      y0 = Math.min(y0, pos.y);
      x1 = Math.max(x1, pos.x + sz.w);
      y1 = Math.max(y1, pos.y + sz.h);
    }
    return Number.isFinite(x0) ? { x0, y0, x1, y1 } : null;
  }

  function fitView(ids) {
    const b = bounds(ids);
    const r = canvas.getBoundingClientRect();
    if (!b || r.width === 0) return;
    const pad = 48;
    const k = Math.min(1.2, Math.max(0.08, Math.min((r.width - pad * 2) / (b.x1 - b.x0 + 40), (r.height - pad * 2 - 40) / (b.y1 - b.y0 + 40))));
    view = { k, x: (r.width - (b.x1 - b.x0) * k) / 2 - b.x0 * k, y: (r.height - 40 - (b.y1 - b.y0) * k) / 2 - b.y0 * k };
    applyView();
  }

  function centerOn(id, minZoom = 0.8) {
    const pos = positions.get(id);
    const sz = sizes.get(id);
    const r = canvas.getBoundingClientRect();
    if (!pos || !sz) return;
    const k = Math.max(view.k, minZoom);
    const sideW = side.style.display === "none" ? 0 : 0;
    view = { k, x: (r.width - sideW) / 2 - (pos.x + sz.w / 2) * k, y: r.height / 2 - (pos.y + sz.h / 2) * k };
    applyView();
  }

  let pan = null;
  let drag = null;
  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || e.target.closest(".sd-overlay, .sd-note button")) return;
    if (e.target.closest(".sd-box, .sd-edge")) return;
    pan = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y, moved: false };
    canvas.setPointerCapture(e.pointerId);
    canvas.classList.add("sd-panning");
  });
  canvas.addEventListener("pointermove", (e) => {
    if (drag) {
      const dx = (e.clientX - drag.x) / view.k;
      const dy = (e.clientY - drag.y) / view.k;
      if (Math.abs(dx) + Math.abs(dy) > 2) drag.moved = true;
      if (!drag.moved) return;
      const pos = { x: Math.round(drag.px + dx), y: Math.round(drag.py + dy) };
      positions.set(drag.id, pos);
      drag.el.setAttribute("transform", `translate(${pos.x},${pos.y})`);
      drawEdges();
      return;
    }
    if (!pan) return;
    const dx = e.clientX - pan.x;
    const dy = e.clientY - pan.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) pan.moved = true;
    view = { ...view, x: pan.vx + dx, y: pan.vy + dy };
    applyView();
  });
  const endPointer = () => {
    if (drag) {
      if (drag.moved) {
        manual[drag.id] = positions.get(drag.id);
        if (current()) saveJson(LAYOUTS + current().key, manual);
      } else selectTable(selectedTable === drag.id ? null : drag.id);
      drag = null;
      return;
    }
    if (pan && !pan.moved && selectedTable) selectTable(null);
    pan = null;
    canvas.classList.remove("sd-panning");
  };
  canvas.addEventListener("pointerup", endPointer);
  canvas.addEventListener("pointercancel", endPointer);
  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const r = canvas.getBoundingClientRect();
      if (e.ctrlKey || e.metaKey) zoomAt(Math.exp(-e.deltaY * 0.01), e.clientX - r.left, e.clientY - r.top);
      else {
        view = { ...view, x: view.x - e.deltaX, y: view.y - e.deltaY };
        applyView();
      }
    },
    { passive: false },
  );

  function startDrag(e, id) {
    if (e.button !== 0) return;
    e.stopPropagation();
    const pos = positions.get(id);
    drag = { id, x: e.clientX, y: e.clientY, px: pos.x, py: pos.y, moved: false, el: e.currentTarget };
    canvas.setPointerCapture(e.pointerId);
    // Keep the dragged box on top.
    boxLayer.append(e.currentTarget);
  }

  function selectTable(id) {
    selectedTable = id;
    draw();
  }

  function focusTable(id, center) {
    selectedTable = id;
    draw();
    if (center) centerOn(id);
  }

  function focusByName(name) {
    if (!model) return;
    const wanted = String(name).toLowerCase();
    const t = model.tables.find((x) => x.id.toLowerCase() === wanted || x.name.toLowerCase() === wanted);
    if (!t) return;
    if (schemaFilter && t.schema !== schemaFilter) {
      schemaFilter = t.schema;
      renderTools();
      relayout(false);
    }
    focusTable(t.id, true);
  }

  // Panels -----------------------------------------------------------------------------------------
  function renderTools() {
    colSeg.replaceChildren(
      seg(
        [
          ["all", "All columns", "Show every column"],
          ["keys", "Keys", "Only primary, foreign and unique keys"],
          ["none", "Tables", "Only the table names"],
        ],
        p.columns,
        (v) => {
          setPrefs({ columns: v });
          renderTools();
          relayout(false);
        },
      ),
    );
    notationSeg.replaceChildren(
      seg(
        [
          ["crow", "Crow's foot", "Entity-relationship notation"],
          ["uml", "UML", "Class diagram associations with multiplicities"],
        ],
        p.notation,
        (v) => {
          setPrefs({ notation: v });
          renderTools();
          draw();
        },
      ),
    );
    const schemas = model?.schemas ?? [];
    schemaSelect.style.display = schemas.length > 1 ? "" : "none";
    schemaSelect.replaceChildren(h("option", { value: "" }, "All schemas"), ...schemas.map((sc) => h("option", { value: sc, selected: sc === schemaFilter }, sc)));
    schemaSelect.value = schemaFilter;
    inferredBox.checked = p.inferred;
  }

  function renderStats() {
    if (!model) return;
    const st = stats(model, relations.filter((r) => r.inferred));
    const shown = positions.size;
    stats_.replaceChildren(
      h("b", {}, String(st.tables)),
      " tables · ",
      h("b", {}, String(st.views)),
      " views · ",
      h("b", {}, String(st.relations)),
      " relations",
      ...(st.inferred ? [" · ", h("b", {}, String(st.inferred)), " inferred"] : []),
      ...(shown < model.tables.length ? [` · ${shown} shown`] : []),
    );
  }

  function renderLegend() {
    const mini = (draw_) => {
      const el = s("svg", { width: 54, height: 14, viewBox: "0 0 54 14" });
      const g = s("g", { class: "sd-edge" });
      g.append(s("path", { class: "sd-line", d: "M2,7 H52" }));
      draw_(g);
      el.append(g);
      return el;
    };
    if (p.notation === "uml") {
      legend.replaceChildren(
        h("div", {}, mini((g) => g.append(s("path", { class: "sd-end", d: "M42,2 L52,7 L42,12" }))), "references"),
        h("div", {}, h("code", {}, "*"), " many  ", h("code", {}, "0..1"), " optional  ", h("code", {}, "1"), " exactly one"),
        h("div", {}, mini((g) => g.querySelector("path").setAttribute("style", "stroke-dasharray:5 4;stroke:color-mix(in srgb, var(--color-warning, #e3973a) 70%, transparent)")), "inferred"),
      );
      return;
    }
    legend.replaceChildren(
      h("div", {}, mini((g) => g.append(crowEnd(2, 7, 1, { many: true, optional: true }))), "zero or many"),
      h("div", {}, mini((g) => g.append(crowEnd(52, 7, -1, { many: false, optional: false }))), "exactly one"),
      h("div", {}, mini((g) => g.append(crowEnd(52, 7, -1, { many: false, optional: true }))), "zero or one"),
      h("div", {}, mini((g) => g.querySelector("path").setAttribute("style", "stroke-dasharray:5 4;stroke:color-mix(in srgb, var(--color-warning, #e3973a) 70%, transparent)")), "inferred"),
    );
  }

  function renderSide() {
    const t = selectedTable ? tableById(selectedTable) : null;
    if (!t) {
      side.style.display = model ? "" : "none";
      side.classList.add("sd-overview");
      if (model) side.replaceChildren(...overview());
      return;
    }
    side.style.display = "";
    side.classList.remove("sd-overview");
    const out = relations.filter((r) => r.from === t.id);
    const inc = relations.filter((r) => r.to === t.id && r.from !== t.id);
    const label = (id) => {
      const x = tableById(id);
      return model.schemas.length > 1 && x.schema !== t.schema ? `${x.schema}.${x.name}` : x.name;
    };
    const relRow = (r, other, dir) =>
      h(
        "button",
        { class: "sd-rel", title: edgeTitle(r), onclick: () => focusTable(other, true) },
        icon(dir === "out" ? "arrowRight" : "arrowLeft"),
        h("span", {}, h("code", {}, label(other)), ` via ${r.fromColumns.join(", ")}`),
        r.inferred ? h("span", { class: "sd-tag sd-warn" }, "inferred") : null,
        h("span", { class: "sd-card-tag" }, dir === "out" ? `${r.one ? "1" : "N"}:1` : `1:${r.one ? "1" : "N"}`),
      );
    const slow = new Set(unindexedForeignKeys(model).filter((r) => r.from === t.id).map((r) => r.id));
    side.replaceChildren(
      h(
        "div",
        { class: "sd-side-head" },
        h(
          "div",
          {},
          h("b", {}, model.schemas.length > 1 ? `${t.schema}.${t.name}` : t.name),
          h("span", {}, [t.kind, t.rows !== null ? `~${t.rows.toLocaleString()} rows` : null, `${t.columns.length} columns`].filter(Boolean).join(" · ")),
          t.junction ? h("span", {}, "Join table between ", h("code", {}, label(t.junction[0])), " and ", h("code", {}, label(t.junction[1]))) : null,
          t.comment ? h("span", { style: "color:var(--color-fg-muted)" }, t.comment) : null,
        ),
        h("button", { class: "sd-icon-btn", title: "Close", onclick: () => selectTable(null) }, icon("close")),
      ),
      h(
        "div",
        { class: "sd-sec" },
        h("h4", {}, "Columns"),
        t.columns.map((c) =>
          h(
            "div",
            { class: "sd-colrow", title: c.comment || undefined },
            h("span", { class: `sd-k ${c.pk ? "sd-pk" : c.fk ? "sd-fk" : c.unique ? "sd-uk" : ""}` }, c.pk ? "PK" : c.fk ? "FK" : c.unique ? "UK" : ""),
            h("code", {}, c.name),
            h("small", {}, `${shortType(c.type)}${c.nullable ? "" : " not null"}`),
            c.default ? h("div", { class: "sd-def" }, `= ${c.default}`) : null,
          ),
        ),
      ),
      out.length ? h("div", { class: "sd-sec" }, h("h4", {}, `References (${out.length})`), out.map((r) => relRow(r, r.to, "out"))) : null,
      inc.length ? h("div", { class: "sd-sec" }, h("h4", {}, `Referenced by (${inc.length})`), inc.map((r) => relRow(r, r.from, "in"))) : null,
      t.indexes.length
        ? h(
            "div",
            { class: "sd-sec" },
            h("h4", {}, `Indexes (${t.indexes.length})`),
            t.indexes.map((ix) => h("div", { class: "sd-ix" }, h("span", {}, ix.name), h("span", {}, `(${ix.columns.join(", ")})`), ix.primary ? h("span", { class: "sd-tag" }, "primary") : ix.unique ? h("span", { class: "sd-tag" }, "unique") : null, ix.partial ? h("span", { class: "sd-tag" }, "partial") : null)),
          )
        : null,
      slow.size || (t.kind === "table" && t.primaryKey.length === 0)
        ? h(
            "div",
            { class: "sd-sec" },
            h("h4", {}, "Notes"),
            t.kind === "table" && t.primaryKey.length === 0 ? h("div", { class: "sd-insight" }, icon("alert"), h("span", {}, "No primary key: rows can't be told apart reliably.")) : null,
            [...slow].map((id) => {
              const r = relations.find((x) => x.id === id);
              return h("div", { class: "sd-insight" }, icon("alert"), h("span", {}, "No index on ", h("code", {}, r.fromColumns.join(", ")), `: joins to ${label(r.to)} and its ON DELETE scan this table.`));
            }),
          )
        : null,
    );
  }

  /** The side panel without a selection: what's in the database and what stands out. */
  function overview() {
    const st = stats(model, relations.filter((r) => r.inferred));
    const slow = unindexedForeignKeys(model).filter((r) => positions.has(r.from));
    const hubs = visibleTables()
      .map((t) => ({ t, n: relations.filter((r) => r.to === t.id && r.from !== t.id).length }))
      .filter((x) => x.n > 0)
      .sort((a, b) => b.n - a.n)
      .slice(0, 6);
    const conn = current();
    const loaded = conn ? store.schema(conn.key)?.at : null;
    const link = (id, text) => h("button", { class: "sd-link", onclick: () => focusTable(id, true) }, text ?? tableById(id).name);
    return [
      h("div", { class: "sd-side-head" }, h("div", {}, h("b", { style: "font-family:inherit" }, conn?.meta.name ?? "Database"), h("span", {}, `${KINDS[model.kind].name}${loaded ? ` · read ${new Date(loaded).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}` : ""}`))),
      h(
        "div",
        { class: "sd-sec" },
        h("h4", {}, "Contents"),
        h("div", { class: "sd-ix" }, h("span", {}, `${st.tables} tables, ${st.views} views, ${st.columns} columns`)),
        h("div", { class: "sd-ix" }, h("span", {}, `${st.relations} foreign keys${st.inferred ? `, ${st.inferred} inferred` : ""}`)),
        st.junctions ? h("div", { class: "sd-ix" }, h("span", {}, `${st.junctions} join table${st.junctions === 1 ? "" : "s"} (many-to-many)`)) : null,
        st.isolated ? h("div", { class: "sd-ix" }, h("span", {}, `${st.isolated} table${st.isolated === 1 ? "" : "s"} without relations`)) : null,
      ),
      hubs.length ? h("div", { class: "sd-sec" }, h("h4", {}, "Most referenced"), hubs.map(({ t, n }) => h("button", { class: "sd-rel", onclick: () => focusTable(t.id, true) }, h("code", {}, t.name), h("span", { class: "sd-card-tag" }, `${n} ref${n === 1 ? "" : "s"}`)))) : null,
      st.noPrimaryKey.length || slow.length
        ? h(
            "div",
            { class: "sd-sec" },
            h("h4", {}, "Worth a look"),
            st.noPrimaryKey.filter((id) => positions.has(id)).map((id) => h("div", { class: "sd-insight" }, icon("alert"), h("span", {}, link(id), " has no primary key"))),
            slow.map((r) => h("div", { class: "sd-insight" }, icon("alert"), h("span", {}, link(r.from), ` · no index on ${r.fromColumns.join(", ")}`))),
          )
        : null,
      h("div", { class: "sd-sec" }, h("span", { class: "wf-subtle", style: "font-size:11px;line-height:1.5" }, "Click a table for its columns, keys and relations. Drag tables to arrange them; ⌘ + scroll zooms.")),
    ];
  }

  // Export -----------------------------------------------------------------------------------------
  async function exportAs(format) {
    if (!model) return;
    const ids = new Set(positions.keys());
    if (format === "svg") {
      const b = bounds();
      if (!b) return;
      const clone = svg.cloneNode(true);
      const css = getComputedStyle(page);
      const vars = ["--color-fg", "--color-fg-muted", "--color-fg-subtle", "--color-panel", "--color-panel-raised", "--color-border", "--color-border-strong", "--color-accent", "--color-warning", "--color-bg", "--font-mono"].map((v) => `${v}: ${css.getPropertyValue(v)};`).join(" ");
      clone.setAttribute("xmlns", SVG_NS);
      clone.setAttribute("viewBox", `${b.x0 - 30} ${b.y0 - 30} ${b.x1 - b.x0 + 60} ${b.y1 - b.y0 + 60}`);
      clone.setAttribute("width", String(Math.round(b.x1 - b.x0 + 60)));
      clone.setAttribute("height", String(Math.round(b.y1 - b.y0 + 60)));
      clone.removeAttribute("style");
      clone.querySelector("g").setAttribute("transform", "");
      clone.insertBefore(s("style", {}, `svg { ${vars} font-family: ${getComputedStyle(document.body).fontFamily}; } ${CSS.slice(CSS.indexOf("/* Diagram */"), CSS.indexOf("/* Side panel */"))}`), clone.firstChild);
      clone.insertBefore(s("rect", { x: b.x0 - 30, y: b.y0 - 30, width: b.x1 - b.x0 + 60, height: b.y1 - b.y0 + 60, style: `fill:${css.getPropertyValue("--color-panel")}` }), clone.children[1]);
      const name = String(current()?.meta.name ?? "schema").replace(/[^\w.-]+/g, "-");
      try {
        const path = await api.writeFile(`~/Downloads/${name}-schema.svg`, new XMLSerializer().serializeToString(clone));
        api.toast(`Saved ${path}`, "success");
        void api.revealFile(path);
      } catch (err) {
        api.toast(String(err?.message ?? err), "error");
      }
      return;
    }
    const text = format === "plantuml" ? toPlantUml(model, relations, ids) : format === "mermaid-class" ? toMermaidClass(model, relations, ids) : toMermaidEr(model, relations, ids);
    try {
      await navigator.clipboard.writeText(text);
      api.toast(`Copied ${format === "plantuml" ? "PlantUML" : "Mermaid"} for ${ids.size} tables`, "success");
    } catch (err) {
      api.toast(String(err?.message ?? err), "error");
    }
  }

  // Header + empty state ---------------------------------------------------------------------------
  function render() {
    if (conns.length === 0) {
      page.replaceChildren(
        h(
          "div",
          { class: "sd-empty" },
          h("div", { class: "sd-logo" }, icon("logo")),
          h("b", {}, "See your database as a diagram"),
          h("div", { class: "wf-muted", style: "max-width:440px" }, "Connect a database to draw its tables, columns and keys as a class diagram with every relation between them. Only the catalog is read, in a read-only session; credentials stay in your keychain."),
          h("div", { class: "sd-starters" }, Object.keys(KINDS).map((kind) => h("button", { class: "sd-starter", onclick: () => openEditor(null, kind) }, kindTile(kind), KINDS[kind].name))),
        ),
      );
      return;
    }
    if (page.firstChild !== shell) page.replaceChildren(shell);
    connSelect.replaceChildren(...conns.map((c) => h("option", { value: c.key, selected: c.key === selectedKey }, `${c.meta.name || c.key} · ${KINDS[c.meta.kind].short}`)));
    connSelect.value = selectedKey ?? "";
    const c = current();
    subtitle.textContent = c ? target(c.meta) : "";
    renderTools();
    showModel();
  }

  function openEditor(conn, initialKind = "postgres") {
    const editing = !!conn;
    const meta = conn?.meta ?? {};
    let kind = editing ? meta.kind : initialKind;
    const name = h("input", { value: meta.name ?? "", placeholder: "Main", spellcheck: "false" });
    const url = h("input", { class: "sd-mono", type: "password", autocomplete: "off", spellcheck: "false" });
    const path = h("input", { class: "sd-mono", value: meta.path ?? "", placeholder: KINDS.sqlite.placeholder, spellcheck: "false" });
    const message = h("div", { class: "sd-alert", style: "display:none" });
    const urlField = h("label", { class: "sd-field" }, h("span", {}, "Connection URL ", h("small", {}, "· stored in your keychain")), h("div", { class: "sd-input" }, url, h("button", { class: "sd-icon-btn", type: "button", title: "Show", onclick: () => (url.type = url.type === "password" ? "text" : "password") }, icon("eye"))));
    const pathField = h("label", { class: "sd-field" }, h("span", {}, "Database file ", h("small", {}, "· absolute, or relative to the current project")), h("div", { class: "sd-input" }, path));
    const kindButtons = h("div", { class: "sd-kinds", role: "radiogroup" });

    function setKind(next) {
      kind = next;
      urlField.style.display = kind === "sqlite" ? "none" : "";
      pathField.style.display = kind === "sqlite" ? "" : "none";
      url.placeholder = editing && meta.kind === kind ? "Leave empty to keep the current URL" : KINDS[kind].placeholder;
      kindButtons.replaceChildren(...Object.keys(KINDS).map((id) => h("button", { type: "button", role: "radio", disabled: editing, "aria-checked": String(id === kind), class: `sd-kind-opt${id === kind ? " sd-on" : ""}`, onclick: () => setKind(id) }, kindTile(id), KINDS[id].short)));
    }
    setKind(kind);

    function build() {
      const label = name.value.trim() || (kind === "sqlite" ? path.value.trim().split(/[\\/]/).pop()?.replace(/\.\w+$/, "") : parseDbUrl(url.value)?.database) || "";
      if (!label) throw new Error("Give the database a name.");
      const key = editing ? conn.key : keyName(label);
      if (key === "ERD_") throw new Error("Use letters or digits in the name.");
      if (!editing && conns.some((c) => c.key === key)) throw new Error(`A database named ${label} already exists.`);
      const next = { kind, name: label };
      if (kind === "sqlite") {
        if (!path.value.trim()) throw new Error("Enter the path of the database file.");
        const st = api.state();
        return { key, value: "-", meta: { ...next, path: path.value.trim(), base: st.workspaces.find((w) => w.id === st.activeWorkspaceId)?.rootDir ?? "" } };
      }
      const text = url.value.trim();
      if (!text) {
        if (!editing) throw new Error("Paste the connection URL.");
        return { key, value: undefined, meta: { ...meta, ...next } };
      }
      const parts = parseDbUrl(text);
      if (!parts) throw new Error(`That isn't a URL. It looks like ${KINDS[kind].placeholder}`);
      if (parts.kind !== kind) throw new Error(`That's not a ${KINDS[kind].name} URL; it should start with ${{ postgres: "postgres://", mysql: "mysql://", mssql: "sqlserver://" }[kind]}.`);
      const shown = { host: parts.host, port: parts.port, user: parts.user, database: parts.database };
      if (kind === "postgres") return { key, value: text, meta: { ...next, ...shown } };
      if (kind === "mssql") {
        const trustCert = /^(true|yes|1)$/i.test(parts.params.trustservercertificate ?? "");
        return { key, value: parts.password || "-", meta: { ...next, ...shown, hasPassword: !!parts.password, trustCert } };
      }
      return { key, value: parts.password || "-", meta: { ...next, ...shown, hasPassword: !!parts.password } };
    }

    async function save(e) {
      e.preventDefault();
      message.style.display = "none";
      try {
        const c = build();
        await api.vault.set(c.key, c.value, c.meta);
        store.forget(c.key);
        close();
        selectedKey = c.key;
        saveJson(`${PREFS}.last`, c.key);
        await refresh();
      } catch (err) {
        message.className = "sd-alert";
        message.replaceChildren(icon("alert"), String(err?.message ?? err));
        message.style.display = "";
      }
    }

    const close = () => backdrop.remove();
    const backdrop = h(
      "div",
      { class: "wf-dialog-backdrop", onmousedown: (e) => e.target === backdrop && close() },
      h(
        "form",
        { class: "wf-dialog sd-dialog", onsubmit: save, onkeydown: (e) => e.key === "Escape" && (e.stopPropagation(), close()) },
        h(
          "div",
          { class: "sd-dialog-head" },
          h("div", { class: "sd-logo" }, icon("logo")),
          h("div", { style: "flex:1;display:flex;flex-direction:column;gap:2px" }, h("h2", { class: "wf-h2", style: "font-weight:600" }, editing ? `Edit ${meta.name || conn.key}` : "Add database"), h("div", { class: "wf-subtle" }, "Only the schema is read, never your data.")),
        ),
        h(
          "div",
          { class: "sd-dialog-body" },
          h("div", { class: "sd-field" }, "Database", kindButtons),
          h("label", { class: "sd-field" }, h("span", {}, "Name ", h("small", {}, "· agents use it to pick the database")), h("div", { class: "sd-input" }, name)),
          urlField,
          pathField,
          message,
        ),
        h(
          "div",
          { class: "sd-dialog-foot" },
          h("span", { class: "sd-grow" }),
          h("button", { class: "wf-button wf-ghost", type: "button", onclick: close }, "Cancel"),
          h("button", { class: "wf-button wf-primary", type: "submit" }, editing ? "Save changes" : "Add and draw"),
        ),
      ),
    );
    document.body.append(backdrop);
    (editing ? url : name).focus();
  }

  const unsubscribe = store.subscribe((e) => {
    if (e.type === "focus") {
      if (e.key !== selectedKey && conns.some((c) => c.key === e.key)) {
        selectedKey = e.key;
        render();
      }
      const conn = current();
      if (conn) void load(conn, false, e.table);
    }
  });
  const resize = new ResizeObserver(() => {
    if (model && positions.size && !resize.fitted) {
      resize.fitted = true;
      fitView();
    }
  });
  resize.observe(canvas);
  render();
  void refresh();
  return () => {
    unsubscribe();
    resize.disconnect();
    document.querySelectorAll(".wf-dialog-backdrop").forEach((d) => d.remove());
    page.remove();
    style.remove();
  };
}

function compact(n) {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 1 : 0)}M`;
}
