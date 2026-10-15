import { describe, expect, it } from "vitest";
import { buildCommand, checkReadOnly, formatResult, groupSchema, parseCsv, parseDbUrl, parseResult, parseTsv, statementAt } from "../plugins/db-explorer/index.js";

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
      params: "sslmode=require",
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

  it("groups schema rows by table and formats results for agents", () => {
    const result = parseResult("table_schema,table_name,column_name,data_type,is_nullable\npublic,users,id,integer,NO\npublic,users,name,text,YES\n", "postgres", 10);
    expect(groupSchema(result)).toEqual([
      { schema: "public", name: "users", columns: [{ name: "id", type: "integer", nullable: false }, { name: "name", type: "text", nullable: true }] },
    ]);
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
