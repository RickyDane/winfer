// Databases: read-only access to the user's Postgres, MySQL, SQLite and SQL Server databases, for
// them on a page and for agents as tools. The page has two views of a connection: Query (its
// tables, a query editor, results and a history) and Diagram (its tables, columns, keys and
// relations drawn as a class diagram, with crow's-foot or UML cardinalities).
//
// Connections live in the plugin's vault: a Postgres URL or a MySQL / SQL Server password is a
// secret the command gets as an environment variable, so agents name a connection and never see
// it. The database CLIs (psql, mysql, sqlite3, sqlcmd) run through `api.vault.exec`. Read-only is
// enforced twice: by the database session (Postgres `default_transaction_read_only`, MySQL `SET
// SESSION TRANSACTION READ ONLY`, `sqlite3 -readonly -safe`) and by `checkReadOnly`, which only
// lets one read statement through and refuses the CLIs' own commands (psql `\!`, mysql `system`,
// sqlite `.shell`), which would otherwise run on the user's machine. SQL Server has no read-only
// session, so there `checkReadOnly` is stricter (SELECT / WITH only, and no INSERT, UPDATE,
// DELETE, MERGE, SELECT INTO, EXEC or DDL anywhere in the statement, nor sqlcmd's `:` / `!!`
// commands), sqlcmd runs with variable substitution off, and each query runs in a transaction
// that is rolled back (`msQuery`).
//
// The schema of every view and of the agents' db_schema comes from the same catalog queries
// (`introspectionSql`), read into a model (`buildModel`) with relations inferred from column names
// (`inferRelations`), laid out (`layout`) and exported (Mermaid, PlantUML). These are pure
// functions tested in scripts/db-explorer.test.mjs; `pnpm dev` with `?db` opens the page on a fake
// shop database (src/dev/dbExplorer.ts).

const IS_WIN = typeof navigator !== "undefined" && (/Win/i.test(navigator.platform ?? "") || /Windows/i.test(navigator.userAgent));
const HISTORY = "winfer.db-explorer.history";
const PAGE = "winfer.db-explorer.page";
const LAYOUTS = "winfer.db-explorer.layout.";
const PREFS = "winfer.db-explorer.diagram";
const MAX_HISTORY = 200;
const ROW_LIMITS = [100, 500, 1000, 5000];
/** What an agent sees of a result, at most. */
const AGENT_CHARS = 12_000;
/** What an agent sees of a schema or diagram, at most. */
const AGENT_SCHEMA_CHARS = 16_000;
const ROWS_MARK = "@@WINFER_ROWS";
const FILE_MARK = "@@WINFER_FILE";
/** Separates the columns of a key inside one cell of the catalog queries. */
const US = "\u001f";

export const KINDS = {
  postgres: { name: "PostgreSQL", short: "Postgres", cli: "psql", placeholder: "postgres://user:password@localhost:5432/app" },
  mysql: { name: "MySQL / MariaDB", short: "MySQL", cli: "mysql", placeholder: "mysql://user:password@localhost:3306/app" },
  sqlite: { name: "SQLite", short: "SQLite", cli: "sqlite3", placeholder: "data/app.db" },
  mssql: { name: "SQL Server", short: "SQL Server", cli: "sqlcmd", placeholder: "sqlserver://user:password@localhost:1433/app" },
};

/** Statements each dialect may start with: reads only. */
const READS = {
  postgres: ["select", "with", "explain", "show", "values", "table"],
  mysql: ["select", "with", "explain", "describe", "desc", "show", "values", "table"],
  sqlite: ["select", "with", "explain", "pragma", "values"],
  mssql: ["select", "with"],
};

/** Things a read statement could still use to write or to leave read-only mode. */
const FORBIDDEN = {
  postgres: /\b(set_config|default_transaction_read_only|transaction_read_only|dblink\w*|lo_import|lo_export|pg_terminate_backend|pg_cancel_backend|pg_reload_conf)\b/i,
  mysql: /\binto\s+(outfile|dumpfile)\b|\b(load_file|get_lock|release_lock)\s*\(/i,
  sqlite: /\b(load_extension|writefile|readfile|edit|fts3_tokenizer)\s*\(/i,
  // No read-only session: anything that writes, runs code or changes the session, wherever it is
  // (`WITH x AS (…) DELETE …`, `SELECT … INTO`), plus sequences, which a rollback doesn't undo.
  mssql:
    /\b(insert|update|delete|merge|into|truncate|drop|alter|create|grant|revoke|deny|exec|execute|sp_\w*|xp_\w*|dbcc|bulk|openrowset|opendatasource|openquery|openxml|backup|restore|kill|shutdown|reconfigure|waitfor|use|set|declare|begin|commit|rollback|save|go|readtext|writetext|updatetext)\b|\bnext\s+value\s+for\b/i,
};

// Catalog SQL ------------------------------------------------------------------------------------
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

// sqlcmd also prints its own column headers (`-h -1` can't go with `-y 0`), so both result sets
// name their one column MS_COL and `parseResult` drops each name and its dashed line.
const MS_COL = "winfer_csv";
const MS_HEADER = new RegExp(`^${MS_COL}[ \\t]*\\n-+[ \\t]*$`, "gm");

/** Wraps a SQL Server query so sqlcmd prints a header line, then one CSV line per row. */
function msCsv(tag, { cols, order, sql }) {
  // Catalog names and descriptions have their own collations; CONCAT needs one.
  const field = (c) => `REPLACE(CAST(q.${c} AS nvarchar(max)) COLLATE DATABASE_DEFAULT, N'"', N'""')`;
  const line = `CONCAT(N'"', ${cols.map(field).join(`, N'","', `)}, N'"')`;
  return `/* erd:${tag} */
SET NOCOUNT ON;
SELECT CAST(N'${cols.join(",")}' AS nvarchar(max)) AS ${MS_COL};
SELECT ${line} AS ${MS_COL} FROM (
${sql}
) q
ORDER BY ${order};`;
}

/** sqlcmd's column separator for queries: a character values don't have. */
const MS_SEP = "\x1f";

/**
 * A query as sqlcmd runs it: in a transaction that's rolled back, so a write that got past
 * `checkReadOnly` is undone. NOCOUNT keeps "(n rows affected)" out of the output.
 */
export function msQuery(sql) {
  return `SET NOCOUNT ON;\nSET XACT_ABORT ON;\nBEGIN TRANSACTION;\n${sql}\n;\nIF @@TRANCOUNT > 0 ROLLBACK TRANSACTION;`;
}

SQL.mssql = Object.fromEntries(Object.entries(MS_PART).map(([tag, part]) => [tag, msCsv(tag, part)]));

/** The three catalog queries for a dialect: `{ columns, constraints, indexes }`. */
export function introspectionSql(kind) {
  const sql = SQL[kind];
  if (!sql) throw new Error(`Unknown database kind “${kind}”.`);
  return sql;
}

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
    if (dialect === "mssql" && c === "[") {
      let j = i + 1;
      while (j < n && !(sql[j] === "]" && sql[j + 1] !== "]")) j += sql[j] === "]" ? 2 : 1;
      if (j >= n) {
        open = true;
        code += c + blank(sql.slice(i + 1));
        break;
      }
      code += c + blank(sql.slice(i + 1, j)) + "]";
      i = j + 1;
      continue;
    }
    if (c === "'" || c === '"' || (c === "`" && dialect !== "postgres" && dialect !== "mssql")) {
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
  // sqlcmd reads its commands (`:!! cmd`, `:r file`, `GO`, `EXIT`…) from any line, even in a string.
  if (dialect === "mssql" && /^[ \t]*(:|!!|(go|quit|reset|ed)[ \t]*(\d+[ \t]*)?$|exit\b)/im.test(sql)) return "sqlcmd commands (like :r, !! or GO) aren't allowed. Write plain SQL; use the schema tool to list tables.";
  if (dialect !== "sqlite" && dialect !== "mssql" && code.includes("\\")) return "Backslash commands (like psql's \\d or mysql's \\!) aren't allowed. Write plain SQL; use the schema tool to list tables.";
  if (dialect === "sqlite" && /^[ \t]*\./m.test(code)) return "sqlite3 dot commands (like .tables) aren't allowed. Write plain SQL; use the schema tool to list tables.";
  const statements = splitStatements(sql, dialect);
  if (statements.length === 0) return "There's no query to run.";
  if (statements.length > 1) return "Run one statement at a time.";
  const { code: stmt } = blankSql(statements[0].text, dialect);
  const word = /^[\s(]*([A-Za-z_]+)/.exec(stmt)?.[1]?.toLowerCase() ?? "";
  if (!READS[dialect].includes(word)) {
    return `${word ? word.toUpperCase() : "This statement"} isn't allowed: queries are read-only and only ${READS[dialect].map((w) => w.toUpperCase()).join(", ")}.`;
  }
  const bad = FORBIDDEN[dialect].exec(stmt);
  if (bad) return `${bad[0].trim()} isn't allowed: queries are read-only.`;
  return null;
}


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


/** Synonyms of the SQL Server connection-string keywords this plugin reads (ADO.NET, ODBC, JDBC). */
const MSSQL_KEYS = {
  server: ["server", "data source", "address", "addr", "network address", "servername", "server name"],
  database: ["initial catalog", "database", "databasename", "database name"],
  user: ["user id", "userid", "uid", "user", "username", "user name"],
  password: ["password", "pwd"],
  integrated: ["integrated security", "trusted_connection", "integratedsecurity"],
  trustservercertificate: ["trustservercertificate", "trust server certificate"],
  encrypt: ["encrypt"],
  port: ["port", "portnumber"],
  instance: ["instancename", "instance name"],
};

/**
 * The `key=value;…` pairs of a connection string (ADO.NET / ODBC rules), keys lower-cased with
 * spaces collapsed. A value may be quoted with " or ' (the quote doubled inside) or wrapped in
 * {…} (} doubled inside), so it can hold ; and =.
 */
export function parseKeyValues(text) {
  const out = {};
  const s = String(text);
  let i = 0;
  while (i < s.length) {
    while (i < s.length && /[\s;]/.test(s[i])) i++;
    if (i >= s.length) break;
    const eq = s.indexOf("=", i);
    if (eq < 0) return null;
    const key = s.slice(i, eq).trim().toLowerCase().replace(/\s+/g, " ");
    i = eq + 1;
    while (i < s.length && /[ \t]/.test(s[i])) i++;
    let value = "";
    const open = s[i];
    const close = open === "{" ? "}" : open === '"' || open === "'" ? open : null;
    if (close) {
      i++;
      for (;;) {
        if (i >= s.length) return null;
        if (s[i] === close) {
          if (s[i + 1] === close) {
            value += close;
            i += 2;
            continue;
          }
          i++;
          break;
        }
        value += s[i++];
      }
      while (i < s.length && s[i] !== ";") i++;
    } else {
      const end = s.indexOf(";", i);
      value = s.slice(i, end < 0 ? s.length : end).trim();
      i = end < 0 ? s.length : end;
    }
    if (key) out[key] = value;
  }
  return out;
}

/**
 * A SQL Server connection string as `parseDbUrl` returns a URL: the ADO.NET form copied from
 * appsettings.json or Azure (`Server=tcp:db.example.com,1433;Initial Catalog=app;User ID=me;
 * Password=…;TrustServerCertificate=True`), the ODBC one (`Driver={…};Server=…;Uid=…;Pwd=…`) or
 * JDBC (`jdbc:sqlserver://host:1433;databaseName=app;user=me;password=…`). null if it isn't one.
 */
export function parseConnectionString(text) {
  let rest = String(text).trim();
  let jdbcServer = null;
  const jdbc = /^jdbc:sqlserver:\/\/([^;]*);?/i.exec(rest);
  if (jdbc) {
    jdbcServer = jdbc[1];
    rest = rest.slice(jdbc[0].length);
  }
  const pairs = parseKeyValues(rest);
  if (!pairs) return null;
  const get = (name) => {
    const k = MSSQL_KEYS[name].find((x) => pairs[x] !== undefined);
    return k === undefined ? undefined : pairs[k];
  };
  let server = jdbcServer ?? get("server");
  if (server === undefined) return null;
  // [protocol:]host[\instance][,port]; JDBC writes host[\instance][:port].
  server = server.trim().replace(/^(tcp|np|lpc|admin):/i, "");
  let port = null;
  const comma = jdbcServer === null ? /,\s*(\d+)\s*$/.exec(server) : /:(\d+)$/.exec(server);
  if (comma) {
    port = Number(comma[1]);
    server = server.slice(0, comma.index);
  }
  let [host, instance = ""] = server.split("\\");
  if (/^(\.|\(local\))$/i.test(host)) host = "localhost";
  if (get("port") && /^\d+$/.test(get("port"))) port = Number(get("port"));
  instance = get("instance") ?? instance;
  const integrated = /^(true|yes|sspi)$/i.test(get("integrated") ?? "");
  const params = {};
  for (const name of ["trustservercertificate", "encrypt"]) {
    const v = get(name);
    if (v !== undefined) params[name] = v;
  }
  return {
    kind: "mssql",
    user: integrated ? "" : (get("user") ?? ""),
    password: integrated ? "" : (get("password") ?? ""),
    host,
    port,
    instance,
    database: get("database") ?? "",
    params,
  };
}

/** A database URL, or a SQL Server connection string; null if it's neither. */
export function parseConnection(text) {
  const t = String(text).trim();
  if (/^jdbc:sqlserver:/i.test(t) || (!/^[a-z][\w+.-]*:\/\//i.test(t) && t.includes("="))) return parseConnectionString(t);
  return parseDbUrl(t);
}


/**
 * Reads `buildCommand`'s output into `{ columns, rows, total, truncated }`, keeping at most
 * `maxRows` rows.
 */
export function parseResult(output, kind, maxRows = Infinity) {
  const text = String(output).replace(/\r\n/g, "\n");
  const mark = text.lastIndexOf(ROWS_MARK);
  const lines = mark < 0 ? null : Number(text.slice(mark + ROWS_MARK.length).trim().split(/\s/)[0]);
  const body = (mark < 0 ? text : text.slice(0, mark)).replace(/\n+$/, "");
  if (!body.trim()) return { columns: [], rows: [], total: 0, truncated: false };
  // A query's output has sqlcmd's header and dashed line; the catalog queries' doesn't, or heads
  // it with MS_COL (sqlcmd 15 prints headers with -y 0, go-sqlcmd and sqlcmd 18 don't).
  const [first = "", second = ""] = body.split("\n", 2);
  if (kind === "mssql" && !first.startsWith(MS_COL) && /^[-\x1f \t]*-[-\x1f \t]*$/.test(second)) return parseSqlcmd(body, lines, maxRows);
  // sqlcmd may pad lines, separate result sets with blank lines and head each with its own
  // column name and a dashed line.
  const all = kind === "mysql" ? parseTsv(body) : parseCsv(kind === "mssql" ? body.replace(MS_HEADER, "").replace(/[ \t]+$/gm, "") : body);
  const [columns = [], ...rows] = kind === "mssql" ? all.filter((r) => r.length > 1 || r[0] !== "") : all;
  // All of the output came back: count the parsed rows (a value can span lines). Else estimate.
  const complete = lines === null || Number.isNaN(lines) || lines <= maxRows + 1;
  const total = complete ? rows.length : Math.max(rows.length, lines - 1);
  return { columns, rows: rows.slice(0, maxRows), total, truncated: total > maxRows };
}

/**
 * A query's sqlcmd output (`MS_SEP` between columns, padded): a header line, a dashed line, then
 * the rows. A value with a line break spans lines, so lines join until a row has every column.
 */
function parseSqlcmd(body, lines, maxRows) {
  const [head = "", , ...rest] = body.split("\n");
  const columns = head.split(MS_SEP).map((c) => c.trim());
  const rows = [];
  let cur = null;
  for (const line of rest) {
    if (cur === null && !line.trim() && columns.length > 1) continue;
    cur = cur === null ? line : `${cur}\n${line}`;
    if (cur.split(MS_SEP).length >= columns.length) {
      rows.push(cur.split(MS_SEP).map((c) => c.trim()));
      cur = null;
    }
  }
  if (cur !== null) rows.push(cur.split(MS_SEP).map((c) => c.trim()));
  const complete = lines === null || Number.isNaN(lines) || lines <= maxRows + 2;
  const total = complete ? rows.length : Math.max(rows.length, lines - 2);
  return { columns, rows: rows.slice(0, maxRows), total, truncated: total > maxRows };
}

/** The output file named by a `toFile` command's output, or null. */
export function outputFile(output) {
  const match = new RegExp(`^${FILE_MARK} (.+)$`, "m").exec(String(output));
  return match ? match[1].trim() : null;
}

/** The CLI's error message without the row marker. */
export function errorText(output) {
  const text = String(output);
  const mark = text.lastIndexOf(ROWS_MARK);
  return (mark < 0 ? text : text.slice(0, mark)).trim() || "The query failed without a message.";
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
 * The shell command that runs `sql` on a connection in a read-only session and prints its output
 * (the first `maxRows` rows plus the header, if given, with NULLs shown as NULL), then
 * `@@WINFER_ROWS <lines of output>`, exiting with the CLI's code. `conn` is `{ key, meta }` from
 * the vault. With `toFile`, a successful run's output stays in a temp file instead, named by
 * `@@WINFER_FILE <path>` (vault.exec only returns the last 20,000 characters).
 */
export function buildCommand(conn, sql, { maxRows = null, timeoutSec = 60, win = IS_WIN, toFile = false } = {}) {
  const { key, meta } = conn;
  const ms = Math.max(1, timeoutSec - 2) * 1000;
  const query = maxRows !== null;
  // Lines before the rows: the header (and sqlcmd's dashed line under it).
  const headLines = meta.kind === "mssql" ? 2 : 1;
  if (query && meta.kind === "mssql") sql = msQuery(sql);
  // [program, args, env] with `$KEY` standing for the secret, filled in per shell below.
  let program;
  let args;
  let env = [];
  if (meta.kind === "postgres") {
    program = "psql";
    args = [{ secret: key }, "-X", "-q", "--csv", "-v", "ON_ERROR_STOP=1", ...(query ? ["-P", "null=NULL"] : [])];
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
  } else if (meta.kind === "mssql") {
    // -b: exit with an error code; -y 0: long values in full (not allowed with -h -1, so
    // parseResult drops sqlcmd's column headers).
    program = "sqlcmd";
    // A named instance without a port is found through the SQL Server Browser.
    const server = meta.instance && !meta.port ? `${meta.host || "localhost"}\\${meta.instance}` : `tcp:${meta.host || "localhost"},${meta.port || 1433}`;
    // -x: no $(variable) substitution, which could print the environment (the password).
    args = ["-S", server, "-b", "-x", "-y", "0", "-l", "10", "-K", "ReadOnly"];
    if (query) args.push("-s", MS_SEP, "-t", String(Math.max(1, timeoutSec - 2)));
    if (meta.database) args.push("-d", meta.database);
    if (meta.user) args.push("-U", meta.user);
    else args.push("-E");
    if (meta.trustCert) args.push("-C");
    if (win) args.push("-f", "65001");
    if (meta.hasPassword) env.push(["SQLCMDPASSWORD", { secret: key }]);
  } else if (meta.kind === "sqlite") {
    program = "sqlite3";
    args = ["-readonly", "-safe", "-bail", "-csv", "-header", ...(query ? ["-nullvalue", "NULL"] : []), "-cmd", ".timeout 5000", meta.path];
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
      toFile ? `if [ $c -eq 0 ]; then printf '${FILE_MARK} %s\\n' "$t"; else cat "$t"; fi` : query ? `head -n ${maxRows + headLines} "$t"` : 'cat "$t"',
      `printf '\\n${ROWS_MARK} %s\\n' "$(wc -l < "$t" | tr -d ' ')"`,
      toFile ? '[ $c -eq 0 ] || rm -f "$t"' : 'rm -f "$t"',
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
    // sqlcmd 15 on Windows reads nothing from a redirected stdin (exit 0, no output): it gets -i.
    ...(meta.kind === "mssql" ? [`$a += ' -i "' + $f.FullName + '"'`] : []),
    `$p = Start-Process -FilePath ${psq(program)} -ArgumentList $a${meta.kind === "mssql" ? "" : " -RedirectStandardInput $f.FullName"} -RedirectStandardOutput $o.FullName -RedirectStandardError $e.FullName -NoNewWindow -Wait -PassThru`,
    toFile
      ? `if ($p.ExitCode -eq 0) { "${FILE_MARK} " + $o.FullName } else { Get-Content $o.FullName -Encoding UTF8 }`
      : `Get-Content $o.FullName${query ? ` -TotalCount ${maxRows + headLines}` : ""} -Encoding UTF8`,
    "Get-Content $e.FullName -Encoding UTF8",
    `"\`n${ROWS_MARK} " + (Get-Content $o.FullName | Measure-Object -Line).Lines`,
    toFile ? "Remove-Item $f.FullName, $e.FullName; if ($p.ExitCode -ne 0) { Remove-Item $o.FullName }" : "Remove-Item $f.FullName, $o.FullName, $e.FullName",
    "exit $p.ExitCode",
  ].join("\n");
}

const PS_PRELUDE = "$ProgressPreference = 'SilentlyContinue'; [Console]::OutputEncoding = [Text.Encoding]::UTF8; $OutputEncoding = [Text.Encoding]::UTF8";
const PS_RUN = "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass";
// cmd refuses longer command lines, and says so on stderr, which vault.exec drops.
const CMD_MAX = 8000;

/**
 * Wraps a PowerShell script so it reaches PowerShell intact through `cmd /C`, or null when it
 * would be too long for cmd's command line.
 */
export function powershell(script) {
  const text = `${PS_PRELUDE}\n${script}`;
  let bytes = "";
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    bytes += String.fromCharCode(c & 0xff, c >> 8);
  }
  const command = `${PS_RUN} -EncodedCommand ${btoa(bytes)}`;
  return command.length <= CMD_MAX ? command : null;
}

/** A PowerShell script file's text that runs `script` once and deletes itself (read whole first). */
export function powershellFile(script) {
  // The BOM makes Windows PowerShell read it as UTF-8, not the ANSI code page.
  return `\uFEFF${PS_PRELUDE}\nRemove-Item -LiteralPath $PSCommandPath -ErrorAction SilentlyContinue\n${script}`;
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

/** One relation as text: `orders(customer_id) → customers(id) [many-to-one, …]`. */
function relationText(r, label) {
  return `${label(r.from)}(${r.fromColumns.join(", ")}) → ${label(r.to)}(${r.toColumns.join(", ")}) [${r.one ? "one-to-one" : "many-to-one"}${r.optional ? ", optional" : ""}${r.onDelete ? `, ON DELETE ${r.onDelete}` : ""}${r.inferred ? ", inferred from the name, no foreign key" : ""}]`;
}

/** A plain-text description for agents: every table with its keys and relations, or one table in full. */
export function describeModel(model, relations, table = null) {
  const byId = new Map(model.tables.map((t) => [t.id, t]));
  const label = (id) => {
    const t = byId.get(id);
    return model.schemas.length > 1 ? `${t.schema}.${t.name}` : t.name;
  };
  const relText = (r) => relationText(r, label);
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

/** Every table's columns and every relation, for an agent about to write a query. */
export function schemaSummary(model, relations) {
  const multi = model.schemas.length > 1;
  const byId = new Map(model.tables.map((t) => [t.id, t]));
  const label = (id) => (multi ? `${byId.get(id).schema}.${byId.get(id).name}` : byId.get(id).name);
  const lines = model.tables
    .filter((t) => t.kind !== "external")
    .map((t) => `${label(t.id)}${t.kind !== "table" ? ` [${t.kind}]` : ""}(${t.columns.map((c) => `${c.name} ${c.type}${c.nullable ? "" : " not null"}${c.pk ? " pk" : ""}`).join(", ")})`);
  if (relations.length) lines.push("", "Relations:", ...relations.map((r) => `  ${relationText(r, label)}`));
  return lines.join("\n");
}

// Plugin ----------------------------------------------------------------------------------------

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

/**
 * Connections, their schemas (read once, shared by the page and agents) and the query history.
 * Listeners get `{ type }` events: "connections", "schema" (with `key`), "history", and "focus"
 * (`key`, `table`) when an agent or the Query view asks the diagram to show a table.
 */
function createStore(api) {
  let history = loadJson(HISTORY, []);
  /** Per connection key: { state: "loading" | "ok" | "error", model?, error?, at?, promise? }. */
  const schemas = new Map();
  const listeners = new Set();
  const emit = (event) => listeners.forEach((fn) => fn(event));
  let home = null;

  /** vault.exec runs in "" for home but doesn't expand "~". */
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

  /** A connection by key or name, for agents; throws listing the known ones. */
  async function find(name) {
    const all = await connections();
    const wanted = String(name ?? "").trim().toLowerCase();
    const hit = all.find((c) => c.key.toLowerCase() === wanted || String(c.meta.name ?? "").toLowerCase() === wanted || keyName(wanted).toLowerCase() === c.key.toLowerCase());
    if (hit) return hit;
    if (!wanted && all.length === 1) return all[0];
    const known = all.map((c) => c.key).join(", ") || "none yet";
    throw new Error(`Unknown connection “${name}”. Connections: ${known}. The user can add one on winfer's Databases page.`);
  }

  /** Runs `sql` as is; `{ code, timedOut, output }`, or throws why it couldn't. */
  async function exec(conn, sql, { maxRows = null, timeoutSec = 60, toFile = false } = {}) {
    const kind = conn.meta.kind;
    const keys = kind === "postgres" || ((kind === "mysql" || kind === "mssql") && conn.meta.hasPassword) ? [conn.key] : [];
    let command = buildCommand(conn, sql, { maxRows, timeoutSec, toFile });
    if (IS_WIN) {
      // The catalog queries are too long for cmd: they go through a script file instead.
      const inline = powershell(command);
      if (inline) command = inline;
      else {
        const file = await api.writeFile(`~/AppData/Local/Temp/winfer-db-${crypto.randomUUID()}.ps1`, powershellFile(command));
        command = `${PS_RUN} -File "${file}"`;
      }
    }
    return api.vault.exec({ command, cwd: await absolute(conn.meta.base ?? ""), keys, timeoutSec });
  }

  /** The CLI's failure as a message, with what to do when the CLI is missing or distrusts the server. */
  function failure(conn, output) {
    const text = errorText(output);
    const { cli, name } = KINDS[conn.meta.kind];
    if (new RegExp(`(${cli}: (command )?not found|not recognized as|cannot find the file)`, "i").test(text)) return `${cli} isn't installed or isn't on the PATH. Install the ${name} command-line client and try again.\n\n${text}`;
    if (conn.meta.kind === "mssql" && /certificate/i.test(text) && !conn.meta.trustCert) return `The server's certificate isn't trusted. For a local or dev server, add ?trustServerCertificate=true to the URL.\n\n${text}`;
    return text;
  }

  /** Runs one read statement; `{ ok, result?, error?, ms }`. */
  async function run(conn, sql, { maxRows = 200, timeoutSec = 60 } = {}) {
    const started = Date.now();
    const problem = checkReadOnly(sql, conn.meta.kind);
    if (problem) return { ok: false, error: problem, ms: 0 };
    try {
      const r = await exec(conn, sql, { maxRows, timeoutSec });
      const ms = Date.now() - started;
      if (r.timedOut) return { ok: false, error: `Stopped after ${timeoutSec} s.`, ms };
      if (r.code !== 0) return { ok: false, error: failure(conn, r.output), ms };
      return { ok: true, result: parseResult(r.output, conn.meta.kind, maxRows), ms };
    } catch (err) {
      return { ok: false, error: String(err?.message ?? err), ms: Date.now() - started };
    }
  }

  /** Runs one catalog query; resolves to `{ columns, rows }` (lower-case columns) or throws. */
  async function catalog(conn, sql, timeoutSec = 90) {
    // A big schema's output is over vault.exec's 20,000 characters, so it's read from its file.
    const r = await exec(conn, sql, { timeoutSec, toFile: true });
    if (r.timedOut) throw new Error(`Reading the schema stopped after ${timeoutSec} s.`);
    if (r.code !== 0) throw new Error(failure(conn, r.output));
    const file = outputFile(r.output);
    if (!file) throw new Error(`Reading the schema gave no output file.\n\n${errorText(r.output)}`);
    let text;
    try {
      text = await api.readFile(file);
    } finally {
      void api.vault.exec({ command: IS_WIN ? `del /f /q "${file}"` : `rm -f ${shq(file)}`, cwd: "", keys: [], timeoutSec: 15 }).catch(() => {});
    }
    const { columns, rows } = parseResult(text.replace(/^\uFEFF/, ""), conn.meta.kind);
    // Every catalog query prints its header, so no output at all means the CLI didn't run it.
    if (columns.length === 0) throw new Error(`${KINDS[conn.meta.kind].cli} exited without an error but printed nothing.`);
    return { columns: columns.map((c) => c.trim().toLowerCase()), rows };
  }

  /** Reads a connection's schema (cached unless `fresh`, or older than `maxAge` ms); resolves to the model. */
  async function load(conn, { fresh = false, maxAge = Infinity } = {}) {
    const have = schemas.get(conn.key);
    if (have && !fresh) {
      if (have.state === "ok" && Date.now() - have.at <= maxAge) return have.model;
      if (have.state === "loading") return have.promise;
    }
    const sql = introspectionSql(conn.meta.kind);
    const promise = (async () => {
      const [columns, constraints, indexes] = await Promise.all([catalog(conn, sql.columns), catalog(conn, sql.constraints), catalog(conn, sql.indexes)]);
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

  function record(entry) {
    history = [{ id: `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`, at: Date.now(), ...entry }, ...history].slice(0, MAX_HISTORY);
    saveJson(HISTORY, history);
    emit({ type: "history" });
  }

  function clearHistory(key) {
    history = key ? history.filter((h) => h.conn !== key) : [];
    saveJson(HISTORY, history);
    emit({ type: "history" });
  }

  return {
    connections,
    find,
    exec,
    run,
    load,
    schema: (key) => schemas.get(key) ?? null,
    forget: (key) => schemas.delete(key),
    record,
    clearHistory,
    history: () => history,
    /** A table the diagram shows when it next opens: `{ key, table }`. */
    pending: null,
    /** Asks the open page to show a table on the diagram. */
    focus: (key, table) => emit({ type: "focus", key, table }),
    emit,
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

const prefs = () => ({ columns: "all", notation: "crow", inferred: true, ...loadJson(PREFS, {}) });
const relationsFor = (model) => [...model.relations, ...(prefs().inferred ? inferRelations(model) : [])];

export function activate(api) {
  const store = createStore(api);
  const clampRows = (v, def) => Math.min(5000, Math.max(1, Math.round(Number(v)) || def));

  api.registerAgentTool({
    name: "db_list",
    description: "List the databases the user connected in winfer (Postgres, MySQL, SQLite, SQL Server), by name, with kind, host and database. Credentials are never shown. Read their structure with db_schema, query them read-only with db_query, draw them with db_diagram.",
    inputSchema: { type: "object", properties: {} },
    async run() {
      const all = await store.connections();
      if (all.length === 0) return "No databases connected yet. Ask the user to add one on winfer's Databases page.";
      return all.map(describe).join("\n");
    },
  });

  api.registerAgentTool({
    name: "db_schema",
    description:
      "List the tables of a connected database with their columns and types and the relations between them (foreign keys and ones inferred from column names). With `table`, that table in full: columns, keys, defaults, indexes, what it references and what references it. Use it before writing a query.",
    inputSchema: {
      type: "object",
      properties: {
        connection: { type: "string", description: "Connection name or key from db_list, e.g. DB_MAIN" },
        table: { type: "string", description: "Only this table, in full (optional)" },
      },
      required: ["connection"],
    },
    async run(args, caller) {
      const conn = await store.find(args.connection);
      const table = args.table ? String(args.table) : "";
      const started = Date.now();
      let model;
      try {
        model = await store.load(conn, { maxAge: 60_000 });
      } catch (err) {
        store.record({ conn: conn.key, sql: `-- schema${table ? ` of ${table}` : ""}`, by: caller.label, error: String(err?.message ?? err), ms: Date.now() - started, schema: true });
        throw err;
      }
      store.record({ conn: conn.key, sql: `-- schema${table ? ` of ${table}` : ""}`, by: caller.label, rows: model.tables.length, ms: Date.now() - started, schema: true });
      if (model.tables.length === 0) return "The database has no tables.";
      const relations = relationsFor(model);
      const text = table ? describeModel(model, relations, table) : schemaSummary(model, relations);
      return text.length > AGENT_SCHEMA_CHARS ? `${text.slice(0, AGENT_SCHEMA_CHARS)}\n… cut; ask for one table with "table".` : text;
    },
  });

  api.registerAgentTool({
    name: "db_query",
    description:
      "Run one read-only SQL statement (SELECT, WITH, EXPLAIN, SHOW…) on a connected database and get the rows back as a table. Writes are refused by the database session (on SQL Server only SELECT and WITH, no INSERT/UPDATE/DELETE/EXEC). Add a LIMIT (TOP on SQL Server) for big tables. Use db_schema first to learn the tables.",
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

  api.registerAgentTool({
    name: "db_diagram",
    description: "Get a database's schema as a diagram in Mermaid (erDiagram or classDiagram) or PlantUML, for docs or to reason about relations. Narrow it to the tables around one table with `table` and `depth`; `show` also opens it for the user.",
    inputSchema: {
      type: "object",
      properties: {
        connection: { type: "string", description: "Connection name or key from db_list" },
        format: { type: "string", enum: ["mermaid-er", "mermaid-class", "plantuml"], description: "Default mermaid-er" },
        table: { type: "string", description: "Only this table and its neighbours (optional)" },
        depth: { type: "number", description: "How many relations away from `table` to include (default 1)" },
        show: { type: "boolean", description: "Also open the diagram on that table for the user" },
      },
      required: ["connection"],
    },
    async run(args) {
      const conn = await store.find(args.connection);
      const model = await store.load(conn, { maxAge: 60_000 });
      const relations = relationsFor(model);
      let ids = null;
      if (args.table) {
        const wanted = String(args.table).toLowerCase();
        const t = model.tables.find((x) => x.id.toLowerCase() === wanted || x.name.toLowerCase() === wanted);
        if (!t) throw new Error(`No table “${args.table}”.`);
        ids = neighbourhood(model, relations, t.id, Math.max(0, Math.min(5, Math.round(Number(args.depth ?? 1)) || 0)));
      }
      if (args.show) {
        store.pending = { key: conn.key, table: args.table ?? null };
        api.openPage("db");
        store.focus(conn.key, args.table ?? null);
      }
      const format = args.format ?? "mermaid-er";
      const text = format === "plantuml" ? toPlantUml(model, relations, ids) : format === "mermaid-class" ? toMermaidClass(model, relations, ids) : toMermaidEr(model, relations, ids);
      return text.length > AGENT_SCHEMA_CHARS ? `${text.slice(0, AGENT_SCHEMA_CHARS)}\n%% cut; narrow it with table and depth` : text;
    },
  });

  api.registerPage({ id: "db", title: "Databases", icon: "database-search", mount: (el) => mountPage(el, api, store) });

  api.registerTool({
    id: "open",
    title: "Databases",
    description: "Open the databases page to browse tables, run read-only SQL queries and see the query history",
    palette: [{ title: "Databases: query a database" }],
    run: () => {
      saveJson(PAGE, { ...loadJson(PAGE, {}), view: "query" });
      store.emit({ type: "view", view: "query" });
      api.openPage("db");
    },
  });

  api.registerTool({
    id: "diagram",
    title: "Database diagram",
    description: "Show the database schema as a class diagram with tables, columns, keys and relations",
    palette: [{ title: "Databases: show schema diagram" }],
    run: () => {
      saveJson(PAGE, { ...loadJson(PAGE, {}), view: "diagram" });
      store.emit({ type: "view", view: "diagram" });
      api.openPage("db");
    },
  });
}

// Page ------------------------------------------------------------------------------------------

const SVG_NS = "http://www.w3.org/2000/svg";

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
  logo: "M3 3h7v5H3zM14 3h7v5h-7zM8.5 16h7v5h-7zM6.5 8v3.5h11V8M12 11.5V16",
  diagram: "M3 3h7v5H3zM14 3h7v5h-7zM8.5 16h7v5h-7zM6.5 8v3.5h11V8M12 11.5V16",
  zoomIn: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3M11 8v6M8 11h6",
  zoomOut: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3M8 11h6",
  fit: "M3 8V3h5M16 3h5v5M21 16v5h-5M8 21H3v-5",
  layout: "M3 3h7v7H3zM14 3h7v7h-7zM14 14h7v7h-7zM3 14h7v7H3z",
  download: "M12 3v12M7 10l5 5 5-5M5 21h14",
  close: "M18 6 6 18M6 6l12 12",
  key: "M15.5 7.5a3.5 3.5 0 1 1-7 0 3.5 3.5 0 0 1 7 0zM12 11v10M12 15h3M12 18h2",
  link: "M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7",
  arrowLeft: "M19 12H5M12 19l-7-7 7-7",
  arrowRight: "M5 12h14M12 5l7 7-7 7",
  eye: "M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
};

function icon(name, cls = "db-icon") {
  const el = document.createElementNS(SVG_NS, "svg");
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("class", cls);
  for (const [k, v] of Object.entries({ fill: "none", stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round", "stroke-linejoin": "round" })) el.setAttribute(k, v);
  el.append(s("path", { d: ICONS[name] }));
  return el;
}

const kindTile = (kind) => h("div", { class: `db-kind db-${kind}` }, kind === "mssql" ? "MS" : KINDS[kind]?.short.slice(0, 2) ?? "DB");

const CSS = `
.db-host { height: 100%; display: flex; flex-direction: column; min-height: 0; container-type: inline-size; }
.db-view { flex: 1; min-height: 0; overflow: auto; }
.db-host-diagram .db-view { overflow: hidden; display: flex; flex-direction: column; }
.db-top { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; padding: 10px 16px; border-bottom: 1px solid var(--color-border); background: var(--color-panel); }
.db-top .db-logo { width: 32px; height: 32px; border-radius: 9px; }
.db-top .db-logo .db-icon { width: 17px; height: 17px; }
.db-top-title { display: flex; flex-direction: column; gap: 1px; min-width: 0; max-width: 300px; margin-right: 4px; }
.db-top-title b { font-size: 14px; font-weight: 600; color: var(--color-fg); }
.db-top-title span { font-size: 11px; color: var(--color-fg-subtle); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.db-top-conn { display: flex; align-items: center; gap: 4px; }
.db-top-conn .db-select { margin-right: 4px; }
.db-top-conn > span:empty { display: none; }
.db-top-conn > span { margin-right: 4px; display: inline-flex; }
.db-grow { flex: 1; }
.db-views { flex-shrink: 0; }
.sd-root { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.db-page { max-width: none; margin: 0; padding: 16px; gap: 16px; }
.db-icon { width: 14px; height: 14px; flex-shrink: 0; }
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
.db-kind.db-mssql { --db-tone: #c94f5d; }

.db-tag { display: inline-flex; align-items: center; gap: 3px; height: 16px; padding: 0 5px; border-radius: 4px; font-size: 10px; font-weight: 500; white-space: nowrap; color: var(--color-fg-subtle); background: var(--color-active); }
.db-tag .db-icon { width: 10px; height: 10px; }
.db-tag.db-ask { color: var(--color-accent); background: var(--color-accent-soft); }
.db-tag.db-auto { color: var(--color-warning); background: color-mix(in srgb, var(--color-warning) 12%, transparent); }
.db-icon-btn { display: inline-flex; align-items: center; justify-content: center; width: 28px; height: 28px; flex-shrink: 0; border-radius: 7px; border: 0; background: transparent; color: var(--color-fg-subtle); cursor: pointer; }
.db-icon-btn:hover { background: var(--color-active); color: var(--color-fg); }
.db-icon-btn.db-bad:hover { color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 12%, transparent); }
.db-icon-btn.db-done { color: var(--color-success); }
.db-icon-btn .db-icon { width: 13px; height: 13px; }

.db-schema { max-height: calc(100vh - 230px); overflow: auto; padding: 4px 0; }
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
.db-select, .sd-select { height: 28px; padding: 0 8px; border-radius: 7px; border: 1px solid var(--color-border); background: var(--color-panel); color: var(--color-fg); font: inherit; font-size: 12px; max-width: 240px; }

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
.db-kinds { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 6px; }
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

/* Diagram view */
.db-icon { width: 14px; height: 14px; flex-shrink: 0; }
.sd-grow { flex: 1; }
.sd-icon-btn { display: inline-flex; align-items: center; justify-content: center; width: 28px; height: 28px; flex-shrink: 0; border-radius: 7px; border: 0; background: transparent; color: var(--color-fg-subtle); cursor: pointer; }
.sd-icon-btn:hover { background: var(--color-active); color: var(--color-fg); }
.sd-icon-btn.sd-bad:hover { color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 12%, transparent); }
.sd-icon-btn:disabled { opacity: .4; pointer-events: none; }
.sd-tools { display: flex; align-items: center; gap: 8px; padding: 8px 16px; border-bottom: 1px solid var(--color-border); flex-wrap: wrap; background: var(--color-panel); }
.sd-search { display: flex; align-items: center; gap: 6px; height: 28px; padding: 0 8px; border-radius: 7px; border: 1px solid var(--color-border); background: var(--color-bg, var(--color-panel)); width: 200px; }
.sd-search .db-icon { width: 12px; height: 12px; color: var(--color-fg-subtle); }
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
.sd-insight .db-icon { margin-top: 1px; color: var(--color-warning, #e3973a); }
.sd-insight code { font-family: var(--font-mono); font-size: 11px; color: var(--color-fg); }
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

/**
 * The Databases page: the Query or Diagram view of the selected connection, which both views
 * share along with the connection dialog, the editor drafts and the last result.
 */
function mountPage(root, api, store) {
  const saved = loadJson(PAGE, {});
  const ctx = {
    api,
    store,
    conns: [],
    selected: saved.conn ?? null,
    view: saved.view === "diagram" ? "diagram" : "query",
    /** Query editor text per connection, kept while the app runs. */
    drafts: new Map(),
    /** The last query run: { conn, ok, result?, error?, ms }. */
    last: null,
    /** Set to run the editor's text when the Query view opens. */
    autorun: false,
    current: () => ctx.conns.find((c) => c.key === ctx.selected) ?? null,
    select(key) {
      ctx.selected = key;
      saveJson(PAGE, { conn: key, view: ctx.view });
      renderHeader();
    },
    /** Set by the open view: how it switches to another connection and reads the schema again. */
    onSelect: null,
    onReload: null,
    /** The views call it when `conns` changed. */
    renderHeader: () => renderHeader(),
    setView(view) {
      if (view === ctx.view && unmount) return;
      ctx.view = view;
      saveJson(PAGE, { conn: ctx.selected, view });
      show();
    },
    /** Opens the Query view on a table's first rows. */
    queryTable(conn, table) {
      const kind = conn.meta.kind;
      const qualify = table.schema && ((kind === "postgres" && table.schema !== "public") || (kind === "mssql" && table.schema !== "dbo"));
      const name = (qualify ? `${quoteName(kind, table.schema)}.` : "") + quoteName(kind, table.name);
      ctx.select(conn.key);
      ctx.drafts.set(conn.key, kind === "mssql" ? `SELECT TOP 100 * FROM ${name}` : `SELECT * FROM ${name} LIMIT 100`);
      ctx.autorun = true;
      ctx.setView("query");
    },
    /** Opens the Diagram view on a table. */
    showInDiagram(conn, table) {
      ctx.select(conn.key);
      store.pending = { key: conn.key, table: table.id };
      ctx.setView("diagram");
    },
    openEditor: (conn, kind) => openEditor(ctx, conn, kind),
    async remove(c) {
      if (!(await api.confirm(`Remove ${c.meta.name || c.key}? Its credentials are deleted from your keychain. The database itself isn't touched.`, { title: "Remove connection", okLabel: "Remove" }))) return;
      try {
        await api.vault.delete(c.key);
        store.forget(c.key);
        ctx.drafts.delete(c.key);
        localStorage.removeItem(LAYOUTS + c.key);
      } catch (err) {
        api.toast(String(err?.message ?? err), "error");
      }
      if (ctx.selected === c.key) ctx.select(null);
      store.emit({ type: "connections" });
    },
    /** The Query / Diagram switch both views put in their header. */
    viewSwitch: () =>
      h(
        "div",
        { class: "sd-seg db-views", role: "tablist" },
        [
          ["query", "Query", "Browse tables and run read-only queries"],
          ["diagram", "Diagram", "See the tables and their relations as a class diagram"],
        ].map(([v, label, title]) => h("button", { class: v === ctx.view ? "sd-on" : "", title, role: "tab", "aria-selected": String(v === ctx.view), onclick: () => ctx.setView(v) }, label)),
      ),
  };

  // The header both views share: the connection, what it allows, the view switch.
  const connSelect = h("select", {
    class: "db-select",
    title: "Connection",
    onchange: (e) => (ctx.onSelect ? ctx.onSelect(e.target.value) : ctx.select(e.target.value)),
  });
  const subtitle = h("span", {});
  const access = h("span", {});
  const iconBtn = (name, title, onclick, extra = "") => h("button", { class: `db-icon-btn ${extra}`, title, "aria-label": title, onclick }, icon(name));
  const connTools = h(
    "div",
    { class: "db-top-conn" },
    connSelect,
    access,
    iconBtn("refresh", "Read the schema again", () => ctx.onReload?.()),
    iconBtn("pencil", "Edit connection", () => ctx.current() && ctx.openEditor(ctx.current())),
    iconBtn("trash", "Remove connection", () => ctx.current() && ctx.remove(ctx.current()), "db-bad"),
  );
  const switchSlot = h("span", { style: "display:contents" });
  const top = h(
    "header",
    { class: "db-top" },
    h("div", { class: "db-logo" }, icon("db")),
    h("div", { class: "db-top-title" }, h("b", {}, "Databases"), subtitle),
    connTools,
    h("span", { class: "db-grow" }),
    h("span", { class: "db-pill", title: "Every session is read-only; agents never see credentials" }, icon("lock"), "Read-only"),
    switchSlot,
    h("button", { class: "wf-button wf-primary", onclick: () => ctx.openEditor(null) }, icon("plus"), "Add connection"),
  );

  function renderHeader() {
    const c = ctx.current();
    connTools.style.display = ctx.conns.length ? "" : "none";
    connSelect.replaceChildren(...ctx.conns.map((x) => h("option", { value: x.key, selected: x.key === ctx.selected }, `${x.meta.name || x.key} · ${KINDS[x.meta.kind].short}`)));
    connSelect.value = ctx.selected ?? "";
    subtitle.textContent = c ? target(c.meta) : "Browse, query and draw your databases. Agents use them by name and never see the credentials.";
    subtitle.title = subtitle.textContent;
    access.replaceChildren(
      !c
        ? ""
        : c.meta.approve !== false
            ? h("span", { class: "db-tag db-ask", title: "Agents need your OK for each query" }, icon("shield"), "Asks")
            : h("span", { class: "db-tag db-auto", title: "Agents query without asking you" }, icon("bolt"), "Auto"),
    );
    switchSlot.replaceChildren(ctx.viewSwitch());
  }

  const style = h("style", {}, CSS);
  const view = h("div", { class: "db-view" });
  const host = h("div", { class: "db-host" }, top, view);
  root.append(style, host);
  let unmount = null;

  function show() {
    unmount?.();
    ctx.onSelect = ctx.onReload = null;
    view.replaceChildren();
    host.className = `db-host db-host-${ctx.view}`;
    renderHeader();
    unmount = (ctx.view === "diagram" ? mountDiagram : mountQuery)(view, ctx);
  }

  const off = store.subscribe((e) => {
    if (e.type === "view") ctx.setView(e.view);
    // An agent asked to show a table: the Diagram view picks it up from `store.pending`.
    else if (e.type === "focus" && ctx.view !== "diagram") ctx.setView("diagram");
  });
  if (store.pending) ctx.view = "diagram";
  show();
  return () => {
    off();
    unmount?.();
    document.querySelectorAll(".wf-dialog-backdrop.db-backdrop").forEach((d) => d.remove());
    host.remove();
    style.remove();
  };
}

const quoteName = (kind, name) =>
  /^[a-z_][a-z0-9_]*$/.test(name) ? name : kind === "mysql" ? `\`${name.replace(/`/g, "``")}\`` : kind === "mssql" ? `[${name.replace(/]/g, "]]")}]` : `"${name.replace(/"/g, '""')}"`;

/** The page without connections: what it does and a button per kind of database. */
function emptyState(ctx) {
  return h(
    "div",
    { class: "wf-card" },
    h(
      "div",
      { class: "db-empty" },
      h("div", { class: "db-logo" }, icon("db")),
      h("div", { class: "db-empty-title" }, "No databases yet"),
      h("div", { class: "wf-muted", style: "max-width:440px" }, "Connect a database to browse its tables, run read-only queries and see its schema as a diagram. Credentials stay in your keychain; agents only get the connection's name."),
      h("div", { class: "db-starters" }, Object.keys(KINDS).map((kind) => h("button", { class: "db-starter", onclick: () => ctx.openEditor(null, kind) }, kindTile(kind), KINDS[kind].name))),
    ),
  );
}

/** The dialog that adds or edits a connection; saving selects it and tells both views. */
function openEditor(ctx, conn, initialKind = "postgres") {
  const { api, store } = ctx;
  const editing = !!conn;
  const meta = conn?.meta ?? {};
  let kind = editing ? meta.kind : initialKind;
  const name = h("input", { value: meta.name ?? "", placeholder: "Main", spellcheck: "false" });
  const url = h("input", { class: "db-mono", type: "password", autocomplete: "off", spellcheck: "false" });
  const showUrl = h("button", { class: "db-icon-btn", type: "button", title: "Show", onclick: () => (url.type = url.type === "password" ? "text" : "password") }, icon("eye"));
  const path = h("input", { class: "db-mono", value: meta.path ?? "", placeholder: KINDS.sqlite.placeholder, spellcheck: "false" });
  const description = h("input", { value: meta.description ?? "", placeholder: "e.g. Staging replica, safe to query" });
  const approveBox = h("input", { type: "checkbox", class: "wf-switch", checked: meta.approve !== false, onchange: () => approveRow.classList.toggle("db-off", !approveBox.checked) });
  const message = h("div", { class: "db-alert", style: "display:none" });
  const urlLabel = h("span", {});
  const urlField = h("label", { class: "db-field" }, urlLabel, h("div", { class: "db-input" }, url, showUrl));
  const pathField = h("label", { class: "db-field" }, h("span", {}, "Database file ", h("small", {}, "· absolute, or relative to the current project")), h("div", { class: "db-input" }, path));
  const kindButtons = h("div", { class: "db-kinds", role: "radiogroup" });
  const approveRow = h(
    "label",
    { class: `db-approve${meta.approve === false ? " db-off" : ""}` },
    icon("shield"),
    h("div", {}, h("b", {}, "Ask before agents query it"), h("span", {}, "You see each query first and can decline. Schema lookups don't ask.")),
    approveBox,
  );
  const mssqlNote = h("div", { class: "wf-subtle", style: "font-size:11px;line-height:1.5" }, "SQL Server has no read-only session, so only SELECT and WITH queries run: INSERT, UPDATE, DELETE, SELECT INTO, EXEC and DDL are refused, and each query is rolled back. For full safety, connect as a user with read-only rights.");

  function setKind(next) {
    kind = next;
    urlField.style.display = kind === "sqlite" ? "none" : "";
    pathField.style.display = kind === "sqlite" ? "" : "none";
    mssqlNote.style.display = kind === "mssql" ? "" : "none";
    urlLabel.replaceChildren(kind === "mssql" ? "Connection string or URL " : "Connection URL ", h("small", {}, "· stored in your keychain"));
    url.placeholder = editing && meta.kind === kind ? "Leave empty to keep the current one" : kind === "mssql" ? "Server=tcp:db.example.com,1433;Initial Catalog=app;User ID=…;Password=…" : KINDS[kind].placeholder;
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
    const parsed = kind === "sqlite" ? null : parseConnection(url.value);
    const label = name.value.trim() || (kind === "sqlite" ? path.value.trim().split(/[\\/]/).pop()?.replace(/\.\w+$/, "") : parsed?.database) || "";
    if (!label) throw new Error("Give the connection a name.");
    const key = editing ? conn.key : keyName(label);
    if (key === "DB_") throw new Error("Use letters or digits in the name.");
    if (!editing && ctx.conns.some((c) => c.key === key)) throw new Error(`A connection named ${label} already exists.`);
    const next = { kind, name: label, description: description.value.trim(), approve: approveBox.checked };
    if (kind === "sqlite") {
      if (!path.value.trim()) throw new Error("Enter the path of the database file.");
      const s = api.state();
      return { key, value: "-", meta: { ...next, path: path.value.trim(), base: s.workspaces.find((w) => w.id === s.activeWorkspaceId)?.rootDir ?? "" } };
    }
    const text = url.value.trim();
    if (!text) {
      if (!editing) throw new Error(kind === "mssql" ? "Paste the connection string." : "Paste the connection URL.");
      return { key, value: undefined, meta: { ...meta, ...next } };
    }
    if (!parsed) throw new Error(`That isn't a ${kind === "mssql" ? "connection string or URL" : "URL"}. It looks like ${kind === "mssql" ? "Server=tcp:host,1433;Initial Catalog=app;User ID=me;Password=…" : KINDS[kind].placeholder}`);
    if (parsed.kind !== kind) throw new Error(`That's not a ${KINDS[kind].name} ${kind === "mssql" ? "connection string" : "URL"}; it should start with ${{ postgres: "postgres://", mysql: "mysql://", mssql: "Server= or sqlserver://" }[kind]}.`);
    const shown = { host: parsed.host, port: parsed.port, user: parsed.user, database: parsed.database };
    // psql takes the whole URL; mysql and sqlcmd take the parts as flags and only the password stays secret.
    if (kind === "postgres") return { key, value: text, meta: { ...next, ...shown } };
    if (kind === "mssql") {
      const trustCert = /^(true|yes|1)$/i.test(parsed.params.trustservercertificate ?? "");
      return { key, value: parsed.password || "-", meta: { ...next, ...shown, instance: parsed.instance ?? "", hasPassword: !!parsed.password, trustCert } };
    }
    return { key, value: parsed.password || "-", meta: { ...next, ...shown, hasPassword: !!parsed.password } };
  }

  async function save(e) {
    e.preventDefault();
    message.style.display = "none";
    try {
      const c = build();
      await api.vault.set(c.key, c.value, c.meta);
      store.forget(c.key);
      close();
      ctx.select(c.key);
      store.emit({ type: "connections" });
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
      const started = Date.now();
      const r = await store.exec({ key: c.key, meta: c.meta }, "SELECT 1", { maxRows: 1, timeoutSec: 20 });
      if (r.timedOut) show("No answer after 20 s.");
      else if (r.code === 0) show(`Connected in ${Date.now() - started} ms.`, true);
      else show(errorText(r.output));
    } catch (err) {
      show(String(err?.message ?? err));
    }
  }

  const close = () => backdrop.remove();
  const backdrop = h(
    "div",
    { class: "wf-dialog-backdrop db-backdrop", onmousedown: (e) => e.target === backdrop && close() },
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
        mssqlNote,
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

/** The Query view: connections, the selected one's tables, a query editor, results and history. */
function mountQuery(root, ctx) {
  const { api, store } = ctx;
  let loadError = "";
  const openTables = new Set();
  let tableFilter = "";
  let maxRows = 500;
  let running = false;
  let copied = false;

  const page = h("div", { class: "wf-page db-page" });
  root.append(page);

  const current = ctx.current;
  const selected = () => ctx.selected;

  async function refresh() {
    try {
      ctx.conns = await store.connections();
      loadError = "";
    } catch (err) {
      loadError = String(err?.message ?? err);
    }
    if (!current()) ctx.select(ctx.conns[0]?.key ?? null);
    ctx.renderHeader();
    editor.value = ctx.drafts.get(selected()) ?? "";
    render();
    const conn = current();
    if (conn) void loadSchema(conn);
    if (conn && ctx.autorun) {
      ctx.autorun = false;
      void runQuery();
    }
  }

  async function loadSchema(conn, fresh = false) {
    try {
      await store.load(conn, { fresh });
    } catch {
      // The store keeps the error; renderSide shows it.
    }
  }

  function select(key) {
    if (selected() === key) return;
    if (selected()) ctx.drafts.set(selected(), editor.value);
    ctx.select(key);
    editor.value = ctx.drafts.get(key) ?? "";
    tableFilter = "";
    filterInput.value = "";
    render();
    const conn = current();
    if (conn) void loadSchema(conn);
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
    ctx.last = { conn: conn.key, ...res };
    store.record({ conn: conn.key, sql, by: "you", rows: res.ok ? res.result.total : undefined, error: res.ok ? undefined : res.error, ms: res.ms });
    renderMain();
  }

  function insert(text) {
    const start = editor.selectionStart;
    editor.setRangeText(text, start, editor.selectionEnd, "end");
    editor.focus();
  }

  function previewTable(conn, table) {
    ctx.queryTable(conn, table);
    editor.value = ctx.drafts.get(conn.key);
    editor.setSelectionRange(0, 0);
    ctx.autorun = false;
    void runQuery();
  }

  const btn = (name, title, onclick, extra = "") =>
    h("button", { class: `db-icon-btn ${extra}`, title, "aria-label": title, onclick: (e) => (e.stopPropagation(), onclick(e)) }, icon(name));

  // Static parts; render() fills them.
  const problem = h("div", { class: "db-alert" });
  const body = h("div");
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
    oninput: () => ctx.drafts.set(selected(), editor.value),
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

  page.append(problem, body);
  ctx.onSelect = select;
  ctx.onReload = () => current() && loadSchema(current(), true);

  const grid = h(
    "div",
    { class: "db-grid" },
    h("div", { class: "db-side" }, schemaLabel, schemaCard),
    h("div", { class: "db-main" }, editorCard, resultCard, historyLabel, historyList),
  );

  function render() {
    problem.replaceChildren(icon("alert"), loadError);
    problem.style.display = loadError ? "" : "none";
    if (ctx.conns.length === 0) {
      body.replaceChildren(emptyState(ctx));
      return;
    }
    if (body.firstChild !== grid) body.replaceChildren(grid);
    renderSide();
    renderMain();
  }

  function renderSide() {
    const conn = current();
    const schema = conn ? store.schema(conn.key) : null;
    const tables = schema?.state === "ok" ? schema.model.tables.filter((t) => t.kind !== "external") : [];
    schemaLabel.replaceChildren(
      "Tables",
      schema?.state === "ok" ? h("span", { style: "font-weight:500;text-transform:none;letter-spacing:0" }, String(tables.length)) : null,
    );
    if (!schema || schema.state === "loading") {
      schemaList.replaceChildren(h("div", { class: "db-note" }, "Loading tables…"));
      return;
    }
    if (schema.state === "error") {
      schemaList.replaceChildren(h("div", { class: "db-note db-err" }, schema.error));
      return;
    }
    const shown = tables.filter((t) => !tableFilter || t.name.toLowerCase().includes(tableFilter) || t.columns.some((c) => c.name.toLowerCase().includes(tableFilter)));
    const multiSchema = schema.model.schemas.length > 1;
    if (shown.length === 0) {
      schemaList.replaceChildren(h("div", { class: "db-note" }, tables.length ? "No table matches." : "No tables."));
      return;
    }
    schemaList.replaceChildren(
      ...shown.flatMap((t) => {
        const id = `${conn.key}:${t.id}`;
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
          btn("diagram", "Show on the diagram", () => ctx.showInDiagram(conn, t)),
          btn("play", "Preview rows", () => previewTable(conn, t)),
        );
        if (!open) return [row];
        return [
          row,
          ...t.columns.map((c) =>
            h("div", { class: "db-col", title: "Insert the column name", onclick: () => insert(quoteName(conn.meta.kind, c.name)) }, h("span", {}, c.name), h("span", {}, `${c.type}${c.nullable ? "" : " ·"}`)),
          ),
        ];
      }),
    );
  }

  function resultView() {
    const last = ctx.last;
    if (running) return h("div", { class: "db-note" }, "Running…");
    const conn = current();
    if (!last || last.conn !== selected()) return h("div", { class: "db-note" }, "Results show up here.");
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
    if (!body.contains(grid)) return;
    const conn = current();
    const last = ctx.last;
    runButton.disabled = !conn || running;
    runButton.replaceChildren(icon("play"), running ? "Running…" : "Run", h("span", { class: "db-kbd" }, "⌘↵"));
    status.textContent = last && last.conn === selected() && !running ? `${last.ok ? "Done" : "Failed"} in ${last.ms.toLocaleString()} ms` : conn ? `${KINDS[conn.meta.kind].short} · read-only` : "";
    resultCard.replaceChildren(resultView());

    const items = store.history().filter((it) => it.conn === selected()).slice(0, 30);
    historyLabel.replaceChildren(
      "History",
      h("span", { class: "db-grow" }),
      items.length ? h("button", { class: "wf-button wf-ghost", style: "height:22px;font-size:11px", onclick: () => store.clearHistory(selected()) }, "Clear") : null,
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
                  ctx.drafts.set(selected(), it.sql);
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
                  it.error ? h("span", { class: "db-bad" }, it.error.split("\n")[0].slice(0, 120)) : it.schema ? `${plural(it.rows ?? 0, "table")} in ${(it.ms ?? 0).toLocaleString()} ms` : `${plural(it.rows ?? 0, "row")} in ${(it.ms ?? 0).toLocaleString()} ms`,
                ),
              ),
            ),
          )),
    );
  }

  const unsubscribe = store.subscribe((e) => {
    if (e.type === "history") renderMain();
    else if (e.type === "schema" && e.key === selected()) renderSide();
    else if (e.type === "connections") void refresh();
  });
  render();
  void refresh();
  return () => {
    unsubscribe();
    if (selected()) ctx.drafts.set(selected(), editor.value);
    page.remove();
  };
}

/** The Diagram view: the selected connection's schema as a pannable class diagram. */
function mountDiagram(root, ctx) {
  const { api, store } = ctx;
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

  const page = h("div", { class: "sd-root" });
  root.append(page);

  const current = ctx.current;
  const setPrefs = (patch) => {
    p = { ...p, ...patch };
    saveJson(PREFS, p);
  };

  // Static parts --------------------------------------------------------------------------------
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
  const shell = h("div", { style: "display:contents" }, tools, body);
  ctx.onSelect = (key) => select(key);
  ctx.onReload = () => current() && load(current(), true);

  // Data ----------------------------------------------------------------------------------------
  async function refresh() {
    try {
      ctx.conns = await store.connections();
    } catch (err) {
      ctx.conns = [];
      api.toast(String(err?.message ?? err), "error");
    }
    const want = store.pending;
    store.pending = null;
    if (want && ctx.conns.some((c) => c.key === want.key)) ctx.select(want.key);
    if (!current()) ctx.select(ctx.conns[0]?.key ?? null);
    render();
    if (current()) await load(current(), false, want?.table ?? null);
  }

  async function load(conn, fresh, focusName = null) {
    try {
      const promise = store.load(conn, { fresh });
      showModel();
      await promise;
    } catch {
      // The store keeps the error; showModel shows it.
    }
    if (conn.key !== ctx.selected) return;
    showModel(true);
    if (focusName) focusByName(focusName);
  }

  function select(key) {
    if (key === ctx.selected) return;
    ctx.select(key);
    selectedTable = null;
    schemaFilter = "";
    render();
    if (current()) void load(current(), false);
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
        t.kind !== "external" ? h("button", { class: "sd-icon-btn", title: "Query its rows", onclick: () => current() && ctx.queryTable(current(), t) }, icon("play")) : null,
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
    if (ctx.conns.length === 0) {
      page.replaceChildren(h("div", { class: "wf-page db-page" }, emptyState(ctx)));
      return;
    }
    if (page.firstChild !== shell) page.replaceChildren(shell);
    ctx.renderHeader();
    renderTools();
    showModel();
  }

  const unsubscribe = store.subscribe((e) => {
    if (e.type === "focus") {
      store.pending = null;
      if (e.key !== ctx.selected && ctx.conns.some((c) => c.key === e.key)) {
        ctx.select(e.key);
        selectedTable = null;
        render();
      }
      const conn = current();
      if (conn) void load(conn, false, e.table);
    } else if (e.type === "connections") void refresh();
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
    page.remove();
  };
}

function compact(n) {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 1 : 0)}M`;
}
