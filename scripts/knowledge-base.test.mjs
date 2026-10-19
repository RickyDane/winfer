import { describe, expect, it } from "vitest";
import { codegraphGraph, compactGraph, nodeByName, nodesInOutput, parseReport, pathText, reportSection } from "../plugins/knowledge-base/index.js";

// A small codegraph dump, as the plugin's SQL query writes it: symbols with their kind, links with
// codegraph's 0–1 confidence.
const node = (id, kind, name, file, line = 1) => ({ id, kind, name, source_file: file, source_location: `L${line}` });
const dump = {
  directed: true,
  built_at_commit: "abc1234def",
  nodes: [
    node("f:app", "file", "app.ts", "src/app.ts"),
    node("fn:main", "function", "main", "src/app.ts", 3),
    node("f:db", "file", "db.ts", "src/db/db.ts"),
    node("fn:query", "function", "query", "src/db/db.ts", 10),
    node("cls:Pool", "class", "Pool", "src/db/pool.ts", 2),
    node("fn:render", "function", "render", "src/ui/view.tsx", 5),
    node("m:draw", "method", "draw", "src/ui/canvas.ts", 7),
    node("fn:log", "function", "log", "lib/log.ts", 1),
    node("fn:fmt", "function", "fmt", "lib/fmt.ts", 1),
    node("fn:test", "function", "testMain", "tests/app.test.ts", 1),
    node("fn:cfg", "function", "config", "config/index.ts", 1),
  ],
  links: [
    { source: "f:app", target: "fn:main", relation: "contains", c: null, line: 3 },
    { source: "fn:main", target: "fn:query", relation: "calls", c: 0.9, line: 4 },
    { source: "fn:query", target: "cls:Pool", relation: "instantiates", c: 0.7, line: 11 },
    { source: "fn:main", target: "fn:render", relation: "calls", c: 0.95, line: 5 },
    { source: "fn:render", target: "m:draw", relation: "calls", c: 0.3, line: 6 },
    { source: "fn:render", target: "fn:log", relation: "calls", c: 0.9, line: 6 },
    { source: "fn:test", target: "fn:main", relation: "calls", c: 0.9, line: 2 },
    { source: "fn:main", target: "missing", relation: "calls", c: 0.9, line: 9 },
  ],
};

describe("codegraphGraph", () => {
  const { graph, report } = codegraphGraph(dump, { title: "demo", version: "1.6.2" });

  it("labels functions and methods like graphify and keeps every link between known nodes", () => {
    const labels = Object.fromEntries(graph.nodes.map((n) => [n.id, n.label]));
    expect(labels["fn:main"]).toBe("main()");
    expect(labels["m:draw"]).toBe("draw()");
    expect(labels["cls:Pool"]).toBe("Pool");
    expect(labels["f:app"]).toBe("app.ts");
    expect(graph.links).toHaveLength(7);
    expect(graph.built_at_commit).toBe("abc1234def");
  });

  it("maps confidence to graphify's labels", () => {
    const conf = Object.fromEntries(graph.links.map((l) => [`${l.source}>${l.target}`, l.confidence]));
    expect(conf["fn:main>fn:query"]).toBe("EXTRACTED");
    expect(conf["f:app>fn:main"]).toBe("EXTRACTED");
    expect(conf["fn:query>cls:Pool"]).toBe("INFERRED");
    expect(conf["fn:render>m:draw"]).toBe("AMBIGUOUS");
  });

  it("groups nodes into folder communities, deep enough for a handful", () => {
    const comm = Object.fromEntries(graph.nodes.map((n) => [n.id, n.community]));
    // One level (src, lib, tests, config) is too few; two levels separate src/db from src/ui.
    expect(comm["fn:query"]).toBe(comm["cls:Pool"]);
    expect(comm["fn:query"]).not.toBe(comm["fn:render"]);
    expect(comm["fn:main"]).not.toBe(comm["fn:query"]);
    expect(new Set(Object.values(comm)).size).toBeGreaterThanOrEqual(6);
  });

  it("writes a report parseReport and reportSection read", () => {
    const r = parseReport(report);
    expect(r.title).toBe("demo");
    expect(r.commit).toBe("abc1234def");
    expect(r.god[0]).toEqual({ label: "main()", edges: 4 });
    expect(r.names.size).toBe(new Set(graph.nodes.map((n) => n.community)).size);
    expect(r.summary[0]).toMatch(/^11 nodes · 7 edges · \d+ communities \(by folder\)$/);
    expect(r.surprising.length).toBeGreaterThan(0);
    expect(reportSection(report, "god_nodes")).toContain("`main()` - 4 edges");
  });

  it("copes with an empty index", () => {
    const empty = codegraphGraph({ nodes: [], links: [] });
    expect(empty.graph.nodes).toEqual([]);
    expect(compactGraph(empty.graph).n).toBe(0);
  });
});

describe("codegraph answers on the loaded graph", () => {
  const { graph, report } = codegraphGraph(dump);
  const g = compactGraph(graph, parseReport(report).names);

  it("finds nodes by label, symbol name or file", () => {
    expect(g.labels[nodeByName(g, "main")]).toBe("main()");
    expect(g.labels[nodeByName(g, "main()")]).toBe("main()");
    expect(g.labels[nodeByName(g, "MAIN")]).toBe("main()");
    expect(g.labels[nodeByName(g, "src/db/db.ts")]).toBe("db.ts");
    expect(nodeByName(g, "nope")).toBe(-1);
  });

  it("kb_path walks the shortest route with relations and directions", () => {
    const text = pathText(g, "testMain", "Pool");
    expect(text.split("\n")[0]).toBe("Shortest path, 3 hops:");
    expect(text).toContain("- testMain() [code] tests/app.test.ts:L1");
    expect(text).toContain("--calls [EXTRACTED]-->");
    expect(text).toContain("--instantiates [INFERRED]-->");
    expect(pathText(g, "Pool", "main")).toContain("<--instantiates [INFERRED]--");
    expect(pathText(g, "nope", "main")).toMatch(/No node named "nope"/);
    expect(pathText(g, "config", "main")).toMatch(/aren't connected/);
  });

  it("nodesInOutput reads codegraph's explore, node and impact output", () => {
    const explore = "- `main` (src/app.ts:3) — 2 callers\n- `query` (src/db/db.ts:10)";
    expect(nodesInOutput(g, explore).map((i) => g.labels[i])).toEqual(["main()", "query()"]);
    const nodeOut = "**render** (function)\n\n**Calls →** draw (src/ui/canvas.ts:7), log (lib/log.ts:1)";
    expect(nodesInOutput(g, nodeOut).map((i) => g.labels[i])).toEqual(["render()", "draw()", "log()"]);
    const impact = 'Impact of changing "query" — 2 affected symbols:\n\nsrc/db/db.ts\n  function    query:10\n\nsrc/app.ts\n  function    main:3\n';
    expect(nodesInOutput(g, impact).map((i) => g.labels[i])).toEqual(["query()", "main()"]);
    expect(nodesInOutput(g, pathText(g, "testMain", "Pool")).map((i) => g.labels[i])).toEqual(["testMain()", "main()", "query()", "Pool"]);
  });
});
