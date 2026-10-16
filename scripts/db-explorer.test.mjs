import { describe, expect, it } from "vitest";
import {
  buildCommand,
  buildModel,
  checkReadOnly,
  describeModel,
  formatResult,
  inferRelations,
  introspectionSql,
  layout,
  measureTable,
  neighbourhood,
  parseConnection,
  parseConnectionString,
  parseCsv,
  parseDbUrl,
  parseResult,
  parseTsv,
  schemaSummary,
  shortType,
  statementAt,
  stats,
  toMermaidClass,
  toMermaidEr,
  toPlantUml,
  unindexedForeignKeys,
} from "../plugins/db-explorer/index.js";

describe("checkReadOnly", () => {
  it("lets one read statement through", () => {
    expect(checkReadOnly("SELECT * FROM users WHERE name = 'x;y'", "postgres")).toBeNull();
    expect(checkReadOnly("  (SELECT 1) UNION (SELECT 2)", "mysql")).toBeNull();
    expect(checkReadOnly("with t as (select 1) select * from t", "sqlite")).toBeNull();
    expect(checkReadOnly("SHOW TABLES", "mysql")).toBeNull();
    expect(checkReadOnly("PRAGMA table_info(users)", "sqlite")).toBeNull();
  });

  it("refuses writes and more than one statement", () => {
    expect(checkReadOnly("DELETE FROM users", "postgres")).toMatch(/DELETE isn't allowed/);
    expect(checkReadOnly("SELECT 1; DROP TABLE users", "postgres")).toMatch(/one statement/);
    expect(checkReadOnly("BEGIN READ WRITE", "postgres")).toMatch(/BEGIN/);
    expect(checkReadOnly("", "postgres")).toMatch(/no query/);
  });

  it("refuses ways out of read-only mode", () => {
    expect(checkReadOnly("SELECT set_config('default_transaction_read_only', 'off', false)", "postgres")).toMatch(/set_config/);
    expect(checkReadOnly("SELECT * FROM users INTO OUTFILE '/tmp/x'", "mysql")).toMatch(/OUTFILE/i);
    expect(checkReadOnly("SELECT writefile('x', 'y')", "sqlite")).toMatch(/writefile/);
  });

  it("refuses the CLIs' own commands outside strings", () => {
    expect(checkReadOnly("SELECT 1 \\! rm -rf ~", "postgres")).toMatch(/Backslash/);
    expect(checkReadOnly("SELECT 1 \\! rm -rf ~", "mysql")).toMatch(/Backslash/);
    expect(checkReadOnly(".shell rm -rf ~", "sqlite")).toMatch(/dot commands/);
    expect(checkReadOnly("system rm -rf ~", "mysql")).toMatch(/SYSTEM isn't allowed/);
    // Inside strings they're just text.
    expect(checkReadOnly("SELECT 'a\\b', $$ \\! $$", "postgres")).toBeNull();
    expect(checkReadOnly("SELECT 'it\\'s \\\\'", "mysql")).toBeNull();
  });

  it("follows Postgres E-strings, where a backslash escapes the quote", () => {
    expect(checkReadOnly("SELECT E'\\' \\! rm'", "postgres")).toBeNull();
    // In a standard string the backslash is literal, so the string ends early.
    expect(checkReadOnly("SELECT '\\' \\! rm'", "postgres")).not.toBeNull();
  });

  it("refuses unclosed strings and comments", () => {
    expect(checkReadOnly("SELECT 'oops", "postgres")).toMatch(/isn't closed/);
    expect(checkReadOnly("SELECT 1 /* oops", "sqlite")).toMatch(/isn't closed/);
  });

  it("ignores comments", () => {
    expect(checkReadOnly("-- DELETE FROM x;\nSELECT 1 /* ; drop */", "postgres")).toBeNull();
    expect(checkReadOnly("# note;\nSELECT 1", "mysql")).toBeNull();
  });
});

describe("statementAt", () => {
  const sql = "SELECT 1;\n\nSELECT ';' AS x;\nSELECT 3";
  it("picks the statement under the cursor", () => {
    expect(statementAt(sql, "postgres", 3)).toBe("SELECT 1");
    expect(statementAt(sql, "postgres", sql.indexOf("AS"))).toBe("SELECT ';' AS x");
    expect(statementAt(sql, "postgres", sql.length)).toBe("SELECT 3");
  });
});

describe("parseDbUrl", () => {
  it("reads the parts and decodes them", () => {
    expect(parseDbUrl("postgresql://app:p%40ss@db.example.com:6543/shop?sslmode=require")).toEqual({
      kind: "postgres",
      user: "app",
      password: "p@ss",
      host: "db.example.com",
      port: 6543,
      database: "shop",
      params: { sslmode: "require" },
    });
    expect(parseDbUrl("mysql://root@[::1]/test")).toMatchObject({ kind: "mysql", user: "root", password: "", host: "::1", port: null, database: "test" });
    expect(parseDbUrl("not a url")).toBeNull();
  });
});

describe("parsing output", () => {
  it("reads CSV with quotes, commas and newlines", () => {
    expect(parseCsv('a,b\n1,"x, ""y"""\n2,"multi\nline"\n')).toEqual([
      ["a", "b"],
      ["1", 'x, "y"'],
      ["2", "multi\nline"],
    ]);
  });

  it("reads mysql batch output with escapes", () => {
    expect(parseTsv("a\tb\n1\tx\\ty\\nz\n")).toEqual([
      ["a", "b"],
      ["1", "x\ty\nz"],
    ]);
  });

  it("counts rows and notices when output was cut", () => {
    expect(parseResult("id\n1\n2\n\n@@WINFER_ROWS 3\n", "sqlite", 10)).toEqual({ columns: ["id"], rows: [["1"], ["2"]], total: 2, truncated: false });
    expect(parseResult("id\n1\n2\n\n@@WINFER_ROWS 501\n", "sqlite", 2)).toMatchObject({ rows: [["1"], ["2"]], total: 500, truncated: true });
    expect(parseResult("\n@@WINFER_ROWS 0\n", "postgres", 10)).toMatchObject({ columns: [], total: 0 });
  });

  it("formats results for agents", () => {
    expect(formatResult({ columns: ["a"], rows: [["1"]], total: 3, truncated: true })).toBe("a\n1\n… 2 more rows not shown (add a WHERE or LIMIT, or raise max_rows)");
  });
});

describe("buildCommand", () => {
  it("runs psql read-only with the URL from the vault", () => {
    const cmd = buildCommand({ key: "DB_MAIN", meta: { kind: "postgres" } }, "SELECT 1", { maxRows: 50, timeoutSec: 30, win: false });
    expect(cmd).toContain(`PGOPTIONS='-c default_transaction_read_only=on -c statement_timeout=28000'`);
    expect(cmd).toContain(`psql "$DB_MAIN" '-X'`);
    expect(cmd).toContain(`<<'WINFER_SQL'\nSELECT 1\nWINFER_SQL\n`);
    expect(cmd).toContain('head -n 51 "$t"');
  });

  it("passes mysql's password as MYSQL_PWD and the rest as flags", () => {
    const cmd = buildCommand({ key: "DB_SHOP", meta: { kind: "mysql", host: "db", port: 3307, user: "o'neil", database: "shop", hasPassword: true } }, "SELECT 1", { win: false });
    expect(cmd).toContain(`MYSQL_PWD="$DB_SHOP" mysql`);
    expect(cmd).toContain(`'--init-command=SET SESSION TRANSACTION READ ONLY'`);
    expect(cmd).toContain(`'-u' 'o'\\''neil' 'shop'`);
  });

  it("keeps the heredoc delimiter out of the query", () => {
    const cmd = buildCommand({ key: "DB_T", meta: { kind: "sqlite", path: "a.db" } }, "SELECT 'WINFER_SQL'", { win: false });
    expect(cmd).toContain("<<'WINFER_SQL_X'");
  });

  it("builds a PowerShell script on Windows", () => {
    const cmd = buildCommand({ key: "DB_MAIN", meta: { kind: "postgres" } }, "SELECT 'a'", { win: true });
    expect(cmd).toContain("$q = @'\nSELECT 'a'\n'@");
    expect(cmd).toContain("$env:PGOPTIONS = '-c default_transaction_read_only=on");
    expect(cmd).toContain("Start-Process -FilePath 'psql'");
    expect(() => buildCommand({ key: "DB_MAIN", meta: { kind: "postgres" } }, "SELECT 1\n'@", { win: true })).toThrow();
  });
});

// Schema and diagram ---------------------------------------------------------------------------

const US = "\u001f";
const result = (columns, rows) => ({ columns, rows });
const COLS = ["table_schema", "table_name", "table_type", "column_name", "data_type", "is_nullable", "column_default", "ordinal", "row_estimate", "table_comment", "column_comment"];
const CONS = ["table_schema", "table_name", "constraint_name", "constraint_type", "columns", "ref_schema", "ref_table", "ref_columns", "on_delete", "on_update"];
const IDX = ["table_schema", "table_name", "index_name", "is_unique", "is_primary", "columns", "method", "is_partial"];
const col = (t, c, type, nullable = false, kind = "table") => ["public", t, kind, c, type, nullable ? "YES" : "NO", "", "1", "10", "", ""];

function shop() {
  return buildModel("postgres", {
    columns: result(COLS, [
      col("customers", "id", "integer"),
      col("customers", "email", "text"),
      col("profiles", "customer_id", "integer"),
      col("orders", "id", "integer"),
      col("orders", "customer_id", "integer"),
      col("orders", "coupon_id", "integer", true),
      col("products", "id", "integer"),
      col("order_items", "order_id", "integer"),
      col("order_items", "product_id", "integer"),
      col("order_items", "qty", "integer"),
      col("coupons", "id", "integer"),
      col("shipments", "id", "integer"),
      col("shipments", "order_id", "integer"),
      col("categories", "id", "integer"),
      col("categories", "parent_id", "integer", true),
      col("log", "message", "text"),
      col("recent", "id", "integer", true, "view"),
    ]),
    constraints: result(CONS, [
      ["public", "customers", "customers_pkey", "p", "id", "", "", "", "", ""],
      ["public", "customers", "customers_email_key", "u", "email", "", "", "", "", ""],
      ["public", "profiles", "profiles_pkey", "p", "customer_id", "", "", "", "", ""],
      ["public", "profiles", "profiles_customer_fkey", "f", "customer_id", "public", "customers", "id", "CASCADE", "NO ACTION"],
      ["public", "orders", "orders_pkey", "p", "id", "", "", "", "", ""],
      ["public", "orders", "orders_customer_fkey", "f", "customer_id", "public", "customers", "id", "NO ACTION", "NO ACTION"],
      ["public", "orders", "orders_coupon_fkey", "f", "coupon_id", "public", "coupons", "id", "SET NULL", ""],
      ["public", "products", "products_pkey", "p", "id", "", "", "", "", ""],
      ["public", "coupons", "coupons_pkey", "p", "id", "", "", "", "", ""],
      ["public", "order_items", "order_items_pkey", "p", `order_id${US}product_id`, "", "", "", "", ""],
      ["public", "order_items", "oi_order_fkey", "f", "order_id", "public", "orders", "id", "", ""],
      ["public", "order_items", "oi_product_fkey", "f", "product_id", "public", "products", "", "", ""],
      ["public", "shipments", "shipments_pkey", "p", "id", "", "", "", "", ""],
      ["public", "categories", "categories_pkey", "p", "id", "", "", "", "", ""],
      ["public", "orders", "orders_ext_fkey", "f", "id", "legacy", "orders", "id", "", ""],
    ]),
    indexes: result(IDX, [["public", "orders", "orders_customer_idx", "NO", "NO", "customer_id", "btree", "NO"]]),
  });
}

describe("introspection", () => {
  it("has the three tagged queries per dialect", () => {
    for (const kind of ["postgres", "mysql", "sqlite", "mssql"]) {
      const sql = introspectionSql(kind);
      expect(sql.columns).toMatch(/^\/\* erd:columns \*\//);
      expect(sql.constraints).toMatch(/^\/\* erd:constraints \*\//);
      expect(sql.indexes).toMatch(/^\/\* erd:indexes \*\//);
    }
    expect(() => introspectionSql("oracle")).toThrow(/Unknown/);
  });

  it("runs the CLIs read-only", () => {
    const pg = buildCommand({ key: "DB_X", meta: { kind: "postgres" } }, "SELECT 1", { win: false });
    expect(pg).toContain("default_transaction_read_only=on");
    expect(pg).toContain('psql "$DB_X"');
    const lite = buildCommand({ key: "DB_Y", meta: { kind: "sqlite", path: "a b.db" } }, "SELECT 1", { win: false });
    expect(lite).toContain("'-readonly' '-safe'");
    expect(lite).toContain("'a b.db'");
  });

  it("parses CSV and TSV output of catalog queries", () => {
    expect(parseResult('TABLE_NAME,x\n"a,b",1\n\n@@WINFER_ROWS 2\n', "postgres")).toMatchObject({ columns: ["TABLE_NAME", "x"], rows: [["a,b", "1"]] });
    expect(parseResult("a\tb\nx\\ty\t2\n\n@@WINFER_ROWS 2\n", "mysql")).toMatchObject({ columns: ["a", "b"], rows: [["x\ty", "2"]] });
  });
});

describe("SQL Server", () => {
  it("reads sqlserver:// URLs with their options", () => {
    expect(parseDbUrl("sqlserver://sa:p%40ss@db.local:1444/shop?trustServerCertificate=true")).toMatchObject({ kind: "mssql", user: "sa", password: "p@ss", host: "db.local", port: 1444, database: "shop", params: { trustservercertificate: "true" } });
    expect(parseDbUrl("mssql://localhost/app").kind).toBe("mssql");
  });

  it("runs sqlcmd with the password in the environment", () => {
    const cmd = buildCommand({ key: "DB_MS", meta: { kind: "mssql", host: "db", port: 1444, user: "sa", database: "shop", hasPassword: true, trustCert: true } }, "SELECT 1", { win: false });
    expect(cmd).toContain(`SQLCMDPASSWORD="$DB_MS" sqlcmd '-S' 'tcp:db,1444' '-b' '-y' '0'`);
    expect(cmd).not.toContain("'-h'");
    expect(cmd).toContain("'-d' 'shop' '-U' 'sa' '-C'");
    expect(cmd).not.toContain("p@ss");
    // No user: Windows authentication.
    expect(buildCommand({ key: "DB_MS", meta: { kind: "mssql" } }, "SELECT 1", { win: false })).toContain("'-S' 'tcp:localhost,1433'");
    expect(buildCommand({ key: "DB_MS", meta: { kind: "mssql" } }, "SELECT 1", { win: false })).toContain("'-E'");
  });

  it("builds every row as one CSV line after a header result set", () => {
    const sql = introspectionSql("mssql").constraints;
    expect(sql).toContain("SET NOCOUNT ON;");
    expect(sql).toContain("SELECT CAST(N'table_schema,table_name,constraint_name,constraint_type,columns,ref_schema,ref_table,ref_columns,on_delete,on_update' AS nvarchar(max)) AS winfer_csv;");
    expect(sql).toContain(`REPLACE(CAST(q.on_update AS nvarchar(max)) COLLATE DATABASE_DEFAULT, N'"', N'""')`);
    expect(sql).toContain("FROM sys.foreign_keys fk");
  });

  it("connects to a named instance through the browser service, or a port", () => {
    expect(buildCommand({ key: "DB_MS", meta: { kind: "mssql", host: "localhost", instance: "SQLEXPRESS" } }, "SELECT 1", { win: false })).toContain("'-S' 'localhost\\SQLEXPRESS'");
    expect(buildCommand({ key: "DB_MS", meta: { kind: "mssql", host: "db", instance: "A", port: 1500 } }, "SELECT 1", { win: false })).toContain("'-S' 'tcp:db,1500'");
  });

  it("refuses queries: there's no read-only session", () => {
    expect(checkReadOnly("SELECT 1", "mssql")).toMatch(/aren't supported/);
  });

  it("parses sqlcmd output with padding and blank lines between result sets", () => {
    const out = 'winfer_csv   \n----------   \ntable_schema,table_name   \n\nwinfer_csv\n---------------\n"dbo","say ""hi"", ok"   \n"dbo","two\nlines"\n\n@@WINFER_ROWS 5\n';
    expect(parseResult(out, "mssql")).toMatchObject({ columns: ["table_schema", "table_name"], rows: [["dbo", 'say "hi", ok'], ["dbo", "two\nlines"]] });
  });
});

describe("buildModel", () => {
  const model = shop();
  const rel = (id) => model.relations.find((r) => r.id === id);

  it("reads tables, views, keys and columns", () => {
    const customers = model.tables.find((t) => t.name === "customers");
    expect(customers.primaryKey).toEqual(["id"]);
    expect(customers.columns.find((c) => c.name === "email").unique).toBe(true);
    expect(model.tables.find((t) => t.name === "recent").kind).toBe("view");
    expect(model.tables.find((t) => t.name === "orders").columns.find((c) => c.name === "customer_id")).toMatchObject({ fk: true, indexed: true });
  });

  it("works out cardinality from the keys", () => {
    expect(rel("public.orders:orders_customer_fkey")).toMatchObject({ one: false, optional: false, onDelete: "" });
    expect(rel("public.orders:orders_coupon_fkey")).toMatchObject({ one: false, optional: true, onDelete: "SET NULL" });
    expect(rel("public.profiles:profiles_customer_fkey")).toMatchObject({ one: true, identifying: true, onDelete: "CASCADE" });
  });

  it("points a key without referenced columns at the primary key", () => {
    expect(rel("public.order_items:oi_product_fkey").toColumns).toEqual(["id"]);
  });

  it("adds tables outside the read schemas as external", () => {
    expect(model.tables.find((t) => t.id === "legacy.orders")).toMatchObject({ kind: "external", columns: [{ name: "id" }] });
  });

  it("finds join tables", () => {
    expect(model.tables.find((t) => t.name === "order_items").junction).toEqual(["public.orders", "public.products"]);
    expect(model.tables.find((t) => t.name === "orders").junction).toBeNull();
  });
});

describe("inferRelations", () => {
  it("links *_id columns without a foreign key by name, and parent_id to its own table", () => {
    const inferred = inferRelations(shop());
    expect(inferred.map((r) => `${r.from}.${r.fromColumns[0]}->${r.to}`).sort()).toEqual(["public.categories.parent_id->public.categories", "public.shipments.order_id->public.orders"]);
    expect(inferred.every((r) => r.inferred)).toBe(true);
  });

  it("handles camelCase and -ies plurals", () => {
    const model = buildModel("sqlite", {
      columns: result(COLS, [["main", "categories", "table", "id", "INTEGER", "NO", "", "1", "", "", ""], ["main", "items", "table", "categoryId", "INTEGER", "YES", "", "1", "", "", ""]]),
      constraints: result(CONS, [["main", "categories", "pk", "p", "id", "", "", "", "", ""]]),
      indexes: result(IDX, []),
    });
    expect(inferRelations(model).map((r) => r.to)).toEqual(["main.categories"]);
  });
});

describe("analysis", () => {
  it("counts and flags", () => {
    const model = shop();
    const s = stats(model, inferRelations(model));
    expect(s).toMatchObject({ tables: 9, views: 1, relations: 6, inferred: 2, junctions: 1 });
    expect(s.noPrimaryKey).toEqual(["public.log"]);
    expect(unindexedForeignKeys(model).map((r) => r.name).sort()).toEqual(["oi_product_fkey", "orders_coupon_fkey"]);
  });

  it("describes the whole schema and one table", () => {
    const model = shop();
    const text = describeModel(model, model.relations);
    expect(text).toContain("order_items (join table)");
    expect(text).toContain("Tables without a primary key: public.log");
    const one = describeModel(model, model.relations, "customers");
    expect(one).toContain("Referenced by:");
    expect(one).toContain("public.profiles(customer_id) → public.customers(id) [one-to-one, ON DELETE CASCADE]");
    expect(() => describeModel(model, model.relations, "nope")).toThrow(/No table/);
  });

  it("collects a table's neighbours", () => {
    const model = shop();
    expect([...neighbourhood(model, model.relations, "public.customers", 1)].sort()).toEqual(["public.customers", "public.orders", "public.profiles"]);
  });
});

describe("exports", () => {
  const model = shop();

  it("writes Mermaid ER with crow's-foot cardinality", () => {
    const text = toMermaidEr(model);
    expect(text).toMatch(/^erDiagram/);
    expect(text).toContain('public_customers ||--o{ public_orders : "customer_id"');
    expect(text).toContain('public_coupons |o--o{ public_orders : "coupon_id"');
    expect(text).toContain('public_customers ||--o| public_profiles : "customer_id"');
    expect(text).toContain("integer order_id PK, FK");
  });

  it("writes Mermaid and PlantUML class diagrams", () => {
    expect(toMermaidClass(model)).toContain('public_orders "*" --> "1" public_customers : customer_id');
    expect(toMermaidClass(model)).toContain("<<join table>>");
    const uml = toPlantUml(model);
    expect(uml).toMatch(/^@startuml[\s\S]*@enduml$/);
    expect(uml).toContain('"legacy.orders" <<external>>');
  });

  it("keeps parentheses out of class members", () => {
    const m = buildModel("postgres", { columns: result(COLS, [col("t", "price", "numeric(10,2)")]), constraints: result(CONS, []), indexes: result(IDX, []) });
    expect(toMermaidClass(m)).toContain("-numeric_10-2 price");
  });

  it("shortens long type names", () => {
    expect(shortType("timestamp(3) with time zone")).toBe("timestamptz(3)");
    expect(shortType("character varying(255)")).toBe("varchar(255)");
  });
});

describe("layout", () => {
  it("puts referenced tables left of their referrers and never overlaps boxes", () => {
    const model = shop();
    const relations = [...model.relations, ...inferRelations(model)];
    const sizes = new Map(model.tables.map((t) => [t.id, measureTable(t, "all")]));
    const pos = layout(model.tables, relations, sizes);
    expect(pos.size).toBe(model.tables.length);
    expect(pos.get("public.customers").x).toBeLessThan(pos.get("public.orders").x);
    expect(pos.get("public.orders").x).toBeLessThan(pos.get("public.order_items").x);
    const boxes = [...pos].map(([id, p]) => ({ id, ...p, ...sizes.get(id) }));
    for (const a of boxes) {
      for (const b of boxes) {
        if (a.id >= b.id) continue;
        const overlap = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
        expect(overlap, `${a.id} overlaps ${b.id}`).toBe(false);
      }
    }
  });

  it("sizes boxes by what they show", () => {
    const t = shop().tables.find((x) => x.name === "order_items");
    expect(measureTable(t, "all").h).toBeGreaterThan(measureTable(t, "keys").h);
    expect(measureTable(t, "keys").h).toBeGreaterThan(measureTable(t, "none").h);
  });
});

describe("SQL Server connection strings", () => {
  it("reads the ADO.NET form", () => {
    expect(parseConnection("Server=tcp:10.0.0.5;User ID=dev;Password=s3cret;Initial Catalog=DevDatabase")).toEqual({
      kind: "mssql",
      user: "dev",
      password: "s3cret",
      host: "10.0.0.5",
      port: null,
      instance: "",
      database: "DevDatabase",
      params: {},
    });
    expect(parseConnection("Data Source = db.example.com,1444 ; Initial Catalog=app;User Id=me;Password='it''s;ok';TrustServerCertificate=True;")).toMatchObject({
      host: "db.example.com",
      port: 1444,
      user: "me",
      password: "it's;ok",
      database: "app",
      params: { trustservercertificate: "True" },
    });
  });

  it("reads named instances, local servers and Windows authentication", () => {
    expect(parseConnectionString("Server=.\\SQLEXPRESS;Database=app;Trusted_Connection=yes;User ID=ignored")).toMatchObject({ host: "localhost", instance: "SQLEXPRESS", user: "", password: "", database: "app" });
    expect(parseConnectionString("Server=(local);Integrated Security=SSPI")).toMatchObject({ host: "localhost", user: "" });
  });

  it("reads ODBC and JDBC strings", () => {
    expect(parseConnection("Driver={ODBC Driver 18 for SQL Server};Server=tcp:x.database.windows.net,1433;Uid=me;Pwd={a}}b;c};Database=d;Encrypt=yes")).toMatchObject({ host: "x.database.windows.net", port: 1433, user: "me", password: "a}b;c", database: "d" });
    expect(parseConnection("jdbc:sqlserver://db:1433;databaseName=app;user=sa;password=x")).toMatchObject({ kind: "mssql", host: "db", port: 1433, database: "app", user: "sa", password: "x" });
  });

  it("tells URLs and other text apart", () => {
    expect(parseConnection("postgres://a:b@h/db").kind).toBe("postgres");
    expect(parseConnection("Initial Catalog=app")).toBeNull();
    expect(parseConnection("Server=db;Password='open")).toBeNull();
    expect(parseConnection("hello")).toBeNull();
  });
});

describe("schemaSummary", () => {
  it("lists every table's columns and the relations", () => {
    const model = shop();
    const text = schemaSummary(model, model.relations);
    expect(text).toMatch(/^public\.customers\(id integer not null pk/m);
    expect(text).toContain("Relations:");
  });
});

describe("query mode", () => {
  it("marks NULLs only in a user's query", () => {
    expect(buildCommand({ key: "DB_X", meta: { kind: "postgres" } }, "SELECT 1", { maxRows: 5, win: false })).toContain("'-P' 'null=NULL'");
    expect(buildCommand({ key: "DB_X", meta: { kind: "postgres" } }, "SELECT 1", { win: false })).not.toContain("null=NULL");
    expect(buildCommand({ key: "DB_X", meta: { kind: "sqlite", path: "a.db" } }, "SELECT 1", { win: false })).toContain('cat "$t"');
  });
});
