# Knowledge Base: graphify and codegraph modes

The Knowledge Base plugin (`plugins/knowledge-base`) gets two backends behind the same `kb_*`
agent tools and galaxy page:

- **graphify** (default, today's behaviour): code, docs, papers and images; LLM concepts,
  communities, `GRAPH_REPORT.md`; output in `graphify-out/`.
- **codegraph** ([colbymchenry/codegraph](https://github.com/colbymchenry/codegraph), MIT):
  code only, deterministic tree-sitter + SQLite (`.codegraph/codegraph.db`), no API keys, file
  watcher keeps it fresh. Better for call graphs, blast radius and "which tests does this hit".

## Backend interface

The store, page and agent tools only talk to a backend object:

```js
{
  id, label, outDir,                     // "graphify" | "codegraph", "graphify-out" | ".codegraph"
  caps: { docs, communities, report, explain, path, save, snapshots, deepBuild, exports },
  state,                                 // install state: {state, path, version, step, installError, …}
  bin, detect(), install({ update }),
  metaCommand, stampCommand,             // one shell call: @@NONE | @@STAMP/@@HEAD/@@IGNORED/@@SNAPSHOTS
  read(root),                            // → { graph: node-link JSON text, report: markdown }
  readSnapshot(root, name),
  build(root, { force }),                // quick build → { output, needsForce }
  deepPrompt(update),                    // agent build prompt (graphify only)
  ifGraph(command), noGraph(root),
  commands: { query, explain, path, affected, save },  // → shell command (or null = unsupported)
  report(root), status(root),
}
```

`compactGraph` and the galaxy stay unchanged; the codegraph adapter emits the same node-link shape.

## codegraph adapter

- Install `npm i -g @colbymchenry/codegraph@<pinned>`. Never run `codegraph install` (it edits
  CLAUDE.md/AGENTS.md and agent MCP configs). Run `codegraph telemetry off` on setup.
- Build: `codegraph init` first time, `codegraph sync` after. No deep build. Optional `codegraph daemon`.
- Galaxy: dump `.codegraph/codegraph.db` with `sqlite3 -json` (fallback: CLI `--json`) into a temp
  file, map symbols → nodes, calls/imports/extends → links, community = top-level directory.
- Report synthesized from `codegraph status --json` + fan-in (god nodes) + directories.

| Tool | graphify | codegraph |
|---|---|---|
| `kb_query` | `graphify query` | `codegraph explore` |
| `kb_path` | `graphify path` | BFS over dumped graph in JS |
| `kb_explain` | `graphify explain` | `codegraph node --includeCode` + callers/callees |
| `kb_affected` | `graphify affected` | `codegraph impact --depth` (+ `affected` test files) |
| `kb_report` | `GRAPH_REPORT.md` section | synthesized report section |
| `kb_status` | report head + HEAD | `codegraph status --json` + HEAD |
| `kb_save` | `save-result` | unsupported |

Tool descriptions come from the active backend's caps (codegraph never claims docs).

## Mode selection

`settings.backend` default plus per-root override `settings.backends[root]`. Unset → whichever of
`graphify-out/` / `.codegraph/` exists, else graphify. Switch in the page menu and empty-state card;
hide UI the caps don't support (snapshot timeline, deep build, html exports, docs filter).
Resolve the backend once per agent call.

## Phases

1. **Refactor, no behaviour change** — graphify code behind `createGraphifyBackend`; store, page
   and tools go through it. Dev mock (`src/dev/knowledgeBase.ts`) keeps working.
2. **codegraph adapter** — detect/install/init/sync, `kb_*` mappings, `kb_path` BFS. Start by
   installing the pinned version in a scratch dir and recording the DB schema and `--json` shapes.
3. **Galaxy support** — sqlite dump → node-link JSON, communities, synthesized report.
4. **Mode UI, settings, auto-detect** — `?kb=codegraph` in the dev mock; TESTING.md steps.
5. **Release prep** — plugin `1.5.0`, manifest description/tags, README. `cross-project`
   (`xkb_*`) stays graphify-only for now.

## Risks

- codegraph DB schema and `--json` output are unverified → pin the version, verify in phase 2.
- No `sqlite3` on Windows by default → CLI fallback.
- 100k+ symbols → file-level galaxy past a threshold.
