import { describe, expect, it } from "vitest";
import {
  buildCommand,
  buildModel,
  describeModel,
  inferRelations,
  introspectionSql,
  layout,
  measureTable,
  neighbourhood,
  parseDbUrl,
  parseResult,
  shortType,
  stats,
  toMermaidClass,
  toMermaidEr,
  toPlantUml,
  unindexedForeignKeys,
} from "../plugins/schema-diagram/index.js";

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
    const pg = buildCommand({ key: "ERD_X", meta: { kind: "postgres" } }, "SELECT 1", { win: false });
    expect(pg).toContain("default_transaction_read_only=on");
    expect(pg).toContain('psql "$ERD_X"');
    const lite = buildCommand({ key: "ERD_Y", meta: { kind: "sqlite", path: "a b.db" } }, "SELECT 1", { win: false });
    expect(lite).toContain("'-readonly' '-safe'");
    expect(lite).toContain("'a b.db'");
  });

  it("parses CSV and TSV output with a lower-cased header", () => {
    expect(parseResult('TABLE_NAME,x\n"a,b",1\n\n@@WINFER_ROWS 2\n', "postgres")).toEqual({ columns: ["table_name", "x"], rows: [["a,b", "1"]] });
    expect(parseResult("a\tb\nx\\ty\t2\n\n@@WINFER_ROWS 2\n", "mysql")).toEqual({ columns: ["a", "b"], rows: [["x\ty", "2"]] });
  });
});

describe("SQL Server", () => {
  it("reads sqlserver:// URLs with their options", () => {
    expect(parseDbUrl("sqlserver://sa:p%40ss@db.local:1444/shop?trustServerCertificate=true")).toMatchObject({ kind: "mssql", user: "sa", password: "p@ss", host: "db.local", port: 1444, database: "shop", params: { trustservercertificate: "true" } });
    expect(parseDbUrl("mssql://localhost/app").kind).toBe("mssql");
  });

  it("runs sqlcmd with the password in the environment", () => {
    const cmd = buildCommand({ key: "ERD_MS", meta: { kind: "mssql", host: "db", port: 1444, user: "sa", database: "shop", hasPassword: true, trustCert: true } }, "SELECT 1", { win: false });
    expect(cmd).toContain(`SQLCMDPASSWORD="$ERD_MS" sqlcmd '-S' 'tcp:db,1444' '-b' '-h' '-1' '-y' '0'`);
    expect(cmd).toContain("'-d' 'shop' '-U' 'sa' '-C'");
    expect(cmd).not.toContain("p@ss");
    // No user: Windows authentication.
    expect(buildCommand({ key: "ERD_MS", meta: { kind: "mssql" } }, "SELECT 1", { win: false })).toContain("'-S' 'tcp:localhost,1433'");
    expect(buildCommand({ key: "ERD_MS", meta: { kind: "mssql" } }, "SELECT 1", { win: false })).toContain("'-E'");
  });

  it("builds every row as one CSV line after a header result set", () => {
    const sql = introspectionSql("mssql").constraints;
    expect(sql).toContain("SET NOCOUNT ON;");
    expect(sql).toContain("SELECT CAST(N'table_schema,table_name,constraint_name,constraint_type,columns,ref_schema,ref_table,ref_columns,on_delete,on_update' AS nvarchar(max));");
    expect(sql).toContain(`REPLACE(CAST(q.on_update AS nvarchar(max)) COLLATE DATABASE_DEFAULT, N'"', N'""')`);
    expect(sql).toContain("FROM sys.foreign_keys fk");
  });

  it("parses sqlcmd output with padding and blank lines between result sets", () => {
    const out = 'table_schema,table_name   \n\n"dbo","say ""hi"", ok"   \n"dbo","two\nlines"\n\n@@WINFER_ROWS 5\n';
    expect(parseResult(out, "mssql")).toEqual({ columns: ["table_schema", "table_name"], rows: [["dbo", 'say "hi", ok'], ["dbo", "two\nlines"]] });
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
