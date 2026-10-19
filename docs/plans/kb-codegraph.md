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

## Status

Done in Knowledge Base 1.5.0 (all phases). What changed from the plan once codegraph 1.6.2 was
checked against a real index:

- **Install** with codegraph's official installer (`install.sh` / `install.ps1`, a self-contained
  bundle with its own Node runtime) after a confirm; update with `codegraph upgrade`. npm installs
  are found too. `codegraph install` (agent configs) is never run.
- **Telemetry/update checks** off per run via `CODEGRAPH_TELEMETRY=0 CODEGRAPH_NO_UPDATE_CHECK=1`
  (env, so the user's own `~/.codegraph/telemetry.json` is untouched).
- **Galaxy dump**: one SQL query (`CG_SQL`) builds the node-link JSON; `sqlite3 -readonly` where it
  exists, else the same query through Node's `node:sqlite` (codegraph's bundled node, else `node`).
  Imports, properties and enum members are left out. Stamp = `codegraph.db` mtime (codegraph
  checkpoints its WAL when a run ends).
- **Communities**: folders, as deep as needed for 6+ groups; a folder over 20% of the nodes splits
  into its subfolders; at most 30 (the rest is "Other folders").
- **Agent tools**: `kb_query` → `explore`, `kb_explain` → `node` (file mode for paths), `kb_path` →
  BFS on the loaded graph, `kb_affected` → `impact`, `kb_report` → synthesized report, `kb_status` →
  `status --json` (pending changes = freshness), `kb_save` → unsupported.
- **Engine choice**: `settings.backends[root]`; unset → `.codegraph/` alone means codegraph, else
  graphify. Switch in Settings, on the install/map cards, and the palette ("Knowledge: switch engine").
- **Windows** (1.5.1): codegraph runs as `<bundle>\node.exe --liftoff-only … lib\dist\bin\codegraph.js`,
  like codegraph's own npm launcher, because `codegraph.cmd` passes arguments through cmd.exe (an
  agent's `&`/`%VAR%` would be acted on). Without the bundle, such text is refused. Detection is
  awaited before the first command. Paths are normalized to `/`.
- Tests: `scripts/knowledge-base.test.mjs`, `scripts/knowledge-base-windows.test.mjs` (the PowerShell sent); dev mock `?kb=codegraph`; TESTING.md checklist.

## Phases (original plan)


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
