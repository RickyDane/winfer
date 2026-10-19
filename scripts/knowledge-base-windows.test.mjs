// The Knowledge Base plugin as it runs on Windows: every command is a PowerShell script (sent
// base64-encoded), and codegraph must be started through its bundled node.exe, never codegraph.cmd.
import { beforeAll, describe, expect, it } from "vitest";

const BUNDLE = "C:\\Users\\maya\\AppData\\Local\\codegraph\\current";
let mod;

beforeAll(async () => {
  Object.defineProperty(globalThis, "navigator", { value: { platform: "Win32", userAgent: "Windows NT 10.0" }, configurable: true });
  const store = {};
  globalThis.localStorage = { getItem: (k) => store[k] ?? null, setItem: (k, v) => (store[k] = v) };
  // Imported only now (each test file gets its own modules), so IS_WIN sees the Windows navigator.
  mod = await import("../plugins/knowledge-base/index.js");
});

/** The PowerShell script inside `powershell … -EncodedCommand <base64 UTF-16LE>`. */
const script = (command) => Buffer.from(/-EncodedCommand (\S+)$/.exec(command)[1], "base64").toString("utf16le");

function setup({ bundled = true } = {}) {
  const ran = [];
  const tools = {};
  const api = {
    state: () => ({ workspaces: [{ id: "w", rootDir: "C:\\code\\app", name: "app" }], activeWorkspaceId: "w" }),
    vault: {
      list: async () => [],
      exec: async ({ command }) => {
        const s = script(command);
        ran.push(s);
        if (s.includes("@@GRAPHIFY")) return { code: 0, output: "@@CODEGRAPH\r\n" };
        if (s.includes("Get-Command codegraph")) {
          const found = [`@@BIN ${BUNDLE}\\bin\\codegraph.cmd`, "@@ONPATH 1", "@@VERSION 1.6.2", `@@REAL ${BUNDLE}\\bin\\codegraph.cmd`];
          if (bundled) found.push(`@@NODE ${BUNDLE}\\node.exe`, `@@ENTRY ${BUNDLE}\\lib\\dist\\bin\\codegraph.js`);
          return { code: 0, output: found.join("\r\n") };
        }
        if (s.includes("Get-Command graphify")) return { code: 0, output: "@@MISSING" };
        return { code: 0, output: "ok" };
      },
    },
    registerAgentTool: (t) => (tools[t.name] = t),
    registerPage() {},
    registerTool() {},
    on() {},
    toast() {},
    execute() {},
  };
  mod.activate(api);
  return { ran, tools, caller: { workspaceId: "w", paneId: "p" } };
}

const until = async (fn) => {
  for (let i = 0; i < 50 && !fn(); i++) await new Promise((r) => setTimeout(r, 5));
};

describe("codegraph on Windows", () => {
  it("runs the bundle's node.exe with codegraph's entry, not codegraph.cmd", async () => {
    const { ran, tools, caller } = setup();
    await tools.kb_query.run({ question: 'where is "auth" & %PATH% | billing' }, caller);
    const query = ran.find((s) => s.includes(" explore "));
    expect(query).toContain(`& '${BUNDLE}\\node.exe' --liftoff-only --disable-warning=ExperimentalWarning '${BUNDLE}\\lib\\dist\\bin\\codegraph.js' explore 'where is ''auth'' & %PATH% | billing' --max-files 3`);
    expect(query).not.toContain("codegraph.cmd");
    expect(query).toContain("if (Test-Path '.codegraph/codegraph.db')");
    // The env that turns telemetry and update checks off rides in every script's preamble.
    expect(query).toContain("$env:CODEGRAPH_TELEMETRY = '0'; $env:CODEGRAPH_NO_UPDATE_CHECK = '1'");
  });

  it("refuses text cmd.exe would act on when only codegraph.cmd is there", async () => {
    const { ran, tools, caller } = setup({ bundled: false });
    await until(() => ran.some((s) => s.includes("Get-Command codegraph")));
    await expect(tools.kb_query.run({ question: "a & b" }, caller)).rejects.toThrow(/cmd\.exe/);
    await tools.kb_affected.run({ node: "runTool()", depth: 2 }, caller);
    expect(ran.find((s) => s.includes(" impact "))).toContain(`& '${BUNDLE}\\bin\\codegraph.cmd' impact 'runTool' --depth 2`);
  });

  it("reads the index with node:sqlite through the bundled node.exe", async () => {
    const { ran, tools, caller } = setup();
    await tools.kb_report.run({}, caller).catch(() => {});
    const dump = ran.find((s) => s.includes("winfer-graph.json"));
    expect(dump).toContain(`foreach ($p in @('${BUNDLE}\\node.exe', (Get-Command node -ErrorAction SilentlyContinue).Source))`);
    expect(dump).toMatch(/& \$n -e 'eval\(Buffer\.from\(''[A-Za-z0-9+/=]+'',''base64''\)\.toString\(\)\)' '\.codegraph\/codegraph\.db' '\.codegraph\/winfer-graph\.json'/);
    const js = Buffer.from(/Buffer\.from\(''([A-Za-z0-9+/=]+)''/.exec(dump)[1], "base64").toString();
    expect(js).toContain("require('node:sqlite')");
    expect(js).toContain("json_group_array");
  });
});

describe("codegraph paths from Windows", () => {
  it("normalizes backslashes so folders still become communities", () => {
    const n = (id, file) => ({ id, kind: "function", name: id, source_file: file, source_location: "L1" });
    const { graph } = mod.codegraphGraph({ nodes: [n("a", "src\\ui\\a.ts"), n("b", "src\\ui\\b.ts"), n("c", "src\\db\\c.ts")], links: [] });
    expect(graph.nodes.map((x) => x.source_file)).toEqual(["src/ui/a.ts", "src/ui/b.ts", "src/db/c.ts"]);
    expect(graph.nodes[0].community).toBe(graph.nodes[1].community);
    expect(graph.nodes[0].community).not.toBe(graph.nodes[2].community);
  });
});

describe("codegraph output from Windows", () => {
  it("lights up nodes named with backslash paths", () => {
    const n = (id, name, file) => ({ id, kind: "function", name, source_file: file, source_location: "L1" });
    const { graph, report } = mod.codegraphGraph({ nodes: [n("a", "main", "src\\app.ts"), n("b", "query", "src\\db\\db.ts")], links: [] });
    const g = mod.compactGraph(graph, mod.parseReport(report).names);
    const out = "- `main` (src\\app.ts:1)\n\nsrc\\db\\db.ts\n  function    query:1\n";
    expect(mod.nodesInOutput(g, out).map((i) => g.labels[i])).toEqual(["main()", "query()"]);
  });
});
