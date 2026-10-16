/**
 * Dev-only fake database for the Schema Diagram plugin (plugins/schema-diagram): answers its three
 * catalog queries (tagged `/* erd:columns *\/`, `constraints`, `indexes`) for a made-up shop with
 * two schemas, a join table, a one-to-one, a self reference, a view and relations only the column
 * names give away. Open the dev server with `?erd` to install the plugin with this connection.
 */

export const ERD_DEMO = new URLSearchParams(location.search).has("erd");

export const ERD_CONNECTION = {
  key: "ERD_SHOP",
  value: "postgres://shop:hunter2@localhost:5432/shop",
  meta: { kind: "postgres", name: "Shop", host: "localhost", port: 5432, user: "shop", database: "shop" },
};

type Fk = { cols: string[]; ref: string; refCols: string[]; onDelete?: string };
type Table = {
  t: string;
  kind?: string;
  rows?: number;
  comment?: string;
  /** [name, type, nullable, default?, comment?] */
  cols: [string, string, boolean, string?, string?][];
  pk?: string[];
  uq?: string[][];
  fks?: Fk[];
  idx?: [string, string[]][];
};

const TABLES: Table[] = [
  { t: "public.customers", rows: 12840, comment: "People who signed up", cols: [["id", "integer", false, "nextval('customers_id_seq'::regclass)"], ["email", "character varying(255)", false, "", "Login, lower-cased"], ["name", "text", true], ["created_at", "timestamp with time zone", false, "now()"]], pk: ["id"], uq: [["email"]] },
  { t: "public.customer_profiles", rows: 9120, cols: [["customer_id", "integer", false], ["bio", "text", true], ["avatar_url", "text", true]], pk: ["customer_id"], fks: [{ cols: ["customer_id"], ref: "public.customers", refCols: ["id"], onDelete: "CASCADE" }] },
  { t: "public.addresses", rows: 15210, cols: [["id", "integer", false, "identity"], ["customer_id", "integer", false], ["line1", "text", false], ["city", "text", false], ["country", "character(2)", false]], pk: ["id"], fks: [{ cols: ["customer_id"], ref: "public.customers", refCols: ["id"], onDelete: "CASCADE" }], idx: [["addresses_customer_idx", ["customer_id"]]] },
  { t: "public.categories", rows: 48, cols: [["id", "integer", false, "identity"], ["parent_id", "integer", true], ["name", "text", false], ["slug", "text", false]], pk: ["id"], uq: [["slug"]], fks: [{ cols: ["parent_id"], ref: "public.categories", refCols: ["id"] }] },
  { t: "public.products", rows: 2310, cols: [["id", "integer", false, "identity"], ["category_id", "integer", true], ["sku", "character varying(64)", false], ["name", "text", false], ["price", "numeric(10,2)", false, "0"]], pk: ["id"], uq: [["sku"]], fks: [{ cols: ["category_id"], ref: "public.categories", refCols: ["id"], onDelete: "SET NULL" }], idx: [["products_category_idx", ["category_id"]]] },
  { t: "public.tags", rows: 120, cols: [["id", "integer", false, "identity"], ["name", "text", false]], pk: ["id"], uq: [["name"]] },
  { t: "public.product_tags", rows: 6900, cols: [["product_id", "integer", false], ["tag_id", "integer", false]], pk: ["product_id", "tag_id"], fks: [{ cols: ["product_id"], ref: "public.products", refCols: ["id"], onDelete: "CASCADE" }, { cols: ["tag_id"], ref: "public.tags", refCols: ["id"], onDelete: "CASCADE" }] },
  { t: "public.orders", rows: 48211, cols: [["id", "bigint", false, "identity"], ["customer_id", "integer", false], ["shipping_address_id", "integer", true], ["status", "text", false, "'pending'::text"], ["total", "numeric(10,2)", true], ["created_at", "timestamp with time zone", false, "now()"]], pk: ["id"], fks: [{ cols: ["customer_id"], ref: "public.customers", refCols: ["id"], onDelete: "CASCADE" }, { cols: ["shipping_address_id"], ref: "public.addresses", refCols: ["id"], onDelete: "SET NULL" }], idx: [["orders_customer_idx", ["customer_id"]], ["orders_created_idx", ["created_at"]]] },
  { t: "public.order_items", rows: 131902, cols: [["order_id", "bigint", false], ["product_id", "integer", false], ["qty", "integer", false, "1"], ["unit_price", "numeric(10,2)", false]], pk: ["order_id", "product_id"], fks: [{ cols: ["order_id"], ref: "public.orders", refCols: ["id"], onDelete: "CASCADE" }, { cols: ["product_id"], ref: "public.products", refCols: ["id"] }] },
  { t: "public.reviews", rows: 7410, cols: [["id", "integer", false, "identity"], ["product_id", "integer", false], ["customer_id", "integer", false], ["rating", "smallint", false], ["body", "text", true]], pk: ["id"], fks: [{ cols: ["product_id"], ref: "public.products", refCols: ["id"], onDelete: "CASCADE" }, { cols: ["customer_id"], ref: "public.customers", refCols: ["id"] }], idx: [["reviews_product_idx", ["product_id"]]] },
  { t: "public.shipments", rows: 40102, comment: "Imported from the carrier, no foreign keys", cols: [["id", "integer", false, "identity"], ["order_id", "bigint", false], ["carrier", "text", false], ["tracking_code", "text", true]], pk: ["id"] },
  { t: "public.staff", rows: 34, cols: [["id", "integer", false, "identity"], ["manager_id", "integer", true], ["email", "text", false]], pk: ["id"] },
  { t: "public.audit_log", rows: 902113, cols: [["at", "timestamp with time zone", false, "now()"], ["actor", "text", true], ["message", "text", false]] },
  { t: "public.order_totals", kind: "view", cols: [["customer_id", "integer", true], ["orders", "bigint", true], ["revenue", "numeric", true]] },
  { t: "billing.invoices", rows: 45800, cols: [["id", "uuid", false, "gen_random_uuid()"], ["order_id", "bigint", false], ["issued_at", "timestamp with time zone", false], ["due_at", "date", true]], pk: ["id"], uq: [["order_id"]], fks: [{ cols: ["order_id"], ref: "public.orders", refCols: ["id"] }] },
  { t: "billing.payments", rows: 46012, cols: [["id", "uuid", false, "gen_random_uuid()"], ["invoice_id", "uuid", false], ["amount", "numeric(10,2)", false], ["method", "text", false]], pk: ["id"], fks: [{ cols: ["invoice_id"], ref: "billing.invoices", refCols: ["id"] }], idx: [["payments_invoice_idx", ["invoice_id"]]] },
];

const US = "\u001f";

function csv(rows: (string | number)[][]) {
  return rows.map((r) => r.map((v) => String(v)).map((v) => (/[",\n]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v)).join(",")).join("\n");
}

function part(name: string): (string | number)[][] {
  const split = (id: string) => id.split(".");
  if (name === "columns") {
    return [
      ["table_schema", "table_name", "table_type", "column_name", "data_type", "is_nullable", "column_default", "ordinal", "row_estimate", "table_comment", "column_comment"],
      ...TABLES.flatMap((t) => t.cols.map(([c, type, nullable, def, comment], i) => [...split(t.t), t.kind ?? "table", c, type, nullable ? "YES" : "NO", def ?? "", i + 1, t.rows ?? "", t.comment ?? "", comment ?? ""])),
    ];
  }
  if (name === "constraints") {
    const rows: (string | number)[][] = [["table_schema", "table_name", "constraint_name", "constraint_type", "columns", "ref_schema", "ref_table", "ref_columns", "on_delete", "on_update"]];
    for (const t of TABLES) {
      const [schema, table] = split(t.t);
      if (t.pk) rows.push([schema, table, `${table}_pkey`, "p", t.pk.join(US), "", "", "", "", ""]);
      for (const u of t.uq ?? []) rows.push([schema, table, `${table}_${u.join("_")}_key`, "u", u.join(US), "", "", "", "", ""]);
      for (const f of t.fks ?? []) rows.push([schema, table, `${table}_${f.cols.join("_")}_fkey`, "f", f.cols.join(US), ...split(f.ref), f.refCols.join(US), f.onDelete ?? "NO ACTION", "NO ACTION"]);
    }
    return rows;
  }
  const rows: (string | number)[][] = [["table_schema", "table_name", "index_name", "is_unique", "is_primary", "columns", "method", "is_partial"]];
  for (const t of TABLES) {
    const [schema, table] = split(t.t);
    if (t.pk) rows.push([schema, table, `${table}_pkey`, "YES", "YES", t.pk.join(US), "btree", "NO"]);
    for (const u of t.uq ?? []) rows.push([schema, table, `${table}_${u.join("_")}_key`, "YES", "NO", u.join(US), "btree", "NO"]);
    for (const [ix, cols] of t.idx ?? []) rows.push([schema, table, ix, "NO", "NO", cols.join(US), "btree", "NO"]);
  }
  return rows;
}

export function schemaDiagramExec(command: string) {
  const which = /\/\* erd:(\w+) \*\//.exec(command)?.[1];
  if (!which) return { code: 0, timedOut: false, output: "" };
  const rows = part(which);
  return new Promise((resolve) => setTimeout(() => resolve({ code: 0, timedOut: false, output: `${csv(rows)}\n\n@@WINFER_ROWS ${rows.length}\n` }), 350));
}
