/**
 * Dev-only fake database for the DB Explorer plugin (plugins/db-explorer): answers the psql
 * commands it runs through `vault_exec` with a small made-up shop schema. Open the dev server
 * with `?db` to install DB Explorer and Prompt Library with one Postgres connection.
 */

export const DB_DEMO = new URLSearchParams(location.search).has("db");

export const DB_CONNECTION = {
  key: "DB_SHOP",
  value: "postgres://shop:hunter2@localhost:5432/shop",
  meta: { kind: "postgres", name: "Shop", description: "Local dev database", approve: true, host: "localhost", port: 5432, user: "shop", database: "shop" },
};

const SCHEMA = [
  ["public", "customers", "id", "integer", "NO"],
  ["public", "customers", "email", "text", "NO"],
  ["public", "customers", "name", "text", "YES"],
  ["public", "customers", "created_at", "timestamp with time zone", "NO"],
  ["public", "orders", "id", "integer", "NO"],
  ["public", "orders", "customer_id", "integer", "NO"],
  ["public", "orders", "total", "numeric", "NO"],
  ["public", "orders", "status", "text", "NO"],
  ["public", "order_lines", "order_id", "integer", "NO"],
  ["public", "order_lines", "sku", "text", "NO"],
  ["public", "order_lines", "qty", "integer", "NO"],
];

const ORDERS = Array.from({ length: 42 }, (_, i) => [String(1001 + i), String((i % 7) + 1), (19.5 + i * 3.25).toFixed(2), ["pending", "shipped", "cancelled"][i % 3]]);

function csv(rows: string[][]) {
  return rows.map((r) => r.map((v) => (/[",\n]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v)).join(",")).join("\n");
}

export function dbExplorerExec(command: string) {
  const sql = /<<'WINFER_SQL[_X]*'\n([\s\S]*?)\nWINFER_SQL/.exec(command)?.[1] ?? "";
  const limit = Number(/head -n (\d+)/.exec(command)?.[1] ?? 201) - 1;
  const done = (rows: string[][]) => ({ code: 0, timedOut: false, output: `${csv(rows.slice(0, limit + 1))}\n\n@@WINFER_ROWS ${rows.length}\n` });
  if (/information_schema\.columns/.test(sql)) return done([["table_schema", "table_name", "column_name", "data_type", "is_nullable"], ...SCHEMA]);
  if (/^SELECT 1$/i.test(sql.trim())) return done([["?column?"], ["1"]]);
  if (/from\s+orders/i.test(sql)) return done([["id", "customer_id", "total", "status"], ...ORDERS]);
  if (/from\s+customers/i.test(sql)) return done([["id", "email", "name", "created_at"], ["1", "ada@example.com", "Ada", "2026-09-01 10:12:00+02"], ["2", "linus@example.com", "NULL", "2026-09-03 08:40:00+02"]]);
  return { code: 1, timedOut: false, output: `ERROR:  relation "${/from\s+(\S+)/i.exec(sql)?.[1] ?? "?"}" does not exist\nLINE 1: ${sql.split("\n")[0]}\n\n@@WINFER_ROWS 2\n` };
}
