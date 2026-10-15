# Plugins

A plugin is a folder with a `plugin.json`. Installed plugins live in winfer's plugins folder:

- macOS: `~/Library/Application Support/dev.winfer.app/plugins/<id>/plugin.json`
- Windows: `%APPDATA%\dev.winfer.app\plugins\<id>\plugin.json`

Settings → Plugins is the plugin store: search, install, update, turn on or off and uninstall.
Plugins run with winfer's access to your files, terminals and network, so installing one asks
first, and a folder copied in by hand stays off until you turn it on.

## The store

The store is this repo's [`plugins/`](../../plugins) folder. `plugins/catalog.json` lists every
plugin with the SHA-256 of each file; winfer downloads the files from GitHub
(`raw.githubusercontent.com/RickyDane/winfer/main/plugins`) and refuses any that don't match.
To try the store from a checkout, start winfer with `WINFER_PLUGIN_STORE=/path/to/winfer/plugins`.

To add or change a store plugin:

1. Create or edit `plugins/<id>/` (`id` must equal the folder name; set a `version` and bump it on
   every change, so installed copies offer an update). Optional `tags` help search.
2. Run `node scripts/plugin-catalog.mjs` to rebuild `catalog.json` (a test fails if you forget).

[`plugins/example-hello`](../../plugins/example-hello) uses every manifest feature;
[`plugins/secrets`](../../plugins/secrets) is a full plugin with a page and agent tools.
[`plugins/knowledge-base`](../../plugins/knowledge-base) runs a CLI (graphify) through `vault.exec`,
reads files bigger than its 20,000-character output limit in chunks, and draws a WebGL page.
[`plugins/cross-project`](../../plugins/cross-project) links several projects (say a mobile app, a web
API and the NuGet package with their models) and shows the contracts, endpoints and packages they
share as an SVG map; its analysis is pure functions tested in `scripts/cross-project.test.mjs`, and
`pnpm dev` with `?xp` opens it on three demo projects.
[`plugins/db-explorer`](../../plugins/db-explorer) runs psql, mysql or sqlite3 read-only with the
connection from the vault, and [`plugins/prompt-library`](../../plugins/prompt-library) re-registers
its palette and voice tool whenever the prompts change; `pnpm dev` with `?db` opens both on a fake
database. Their pure functions are tested in `scripts/db-explorer.test.mjs` and
`scripts/prompt-library.test.mjs`.

## plugin.json

| Field | |
|---|---|
| `id` | Required. Lowercase letters, digits, `-` `_` `.`; must equal the folder name. |
| `name` | Required. Shown in Settings. |
| `version` | Required for store plugins; a different version in the catalog shows "Update". |
| `description`, `author`, `tags` | Shown in the store; searched. |
| `providers` | Agent CLIs, same shape as [`src-tauri/resources/providers.json`](../../src-tauri/resources/providers.json). Ids must not clash with built-in agents or other plugins. |
| `recipes` | `{ id, name, panes: [{ provider, prompt? }], preset?, url? }`; listed after your own recipes. |
| `tools` | Declarative actions for the ⌘K palette and voice (below). |
| `main` | A JavaScript ES module inside the folder; its `activate(api)` runs when the plugin is turned on. |

## Tools

A tool is something the user can ask for by voice or pick in the palette:

```jsonc
{
  "id": "run_script",
  "title": "Run package script",                    // palette / HUD label
  "description": "Run an npm script like test, lint or build in a new shell pane", // what voice matches against; ~10–15 words
  "risk": "safe",                                   // or "confirm" to always ask before running by voice
  "params": {
    "script": { "kind": "enum", "ask": "Which script should run?", "options": { "test": "Tests", "lint": "Linter" } }
  },
  "commands": [{ "type": "pane.spawn", "provider": "shell", "prompt": "pnpm {{script}}" }],
  "palette": [{ "title": "Run tests", "args": { "script": "test" } }]   // or true for one entry named after the tool
}
```

`commands` are Command Bus commands (see `Command` in [`src/commands/bus.ts`](../../src/commands/bus.ts)),
for example `pane.spawn`, `pane.sendText`, `pane.broadcast`, `browser.open`, `layout.preset`,
`recipe.load` or `ui.overlay`. `{{param}}` in any string is replaced by that argument.

Param kinds: `enum` (`options`: value → description), `count` (`max`), `bool`, `level` (`levels`),
and `span` (`slot`: `"instruction"` or `"address"`) for free text taken from what the user said.

## Code (`main`)

```js
export function activate(api) {
  api.registerTool({ id: "hi", title: "Say hi", description: "Greet the user", palette: true, run: (args) => api.toast("Hi") });
  api.registerCommand("ping", (args) => api.toast("pong"));     // run with { type: "plugin.run", plugin: "<id>", command: "ping" }
  return api.on("pane.status", (e) => { if (e.status === "done") api.toast(`${e.provider} finished`); });
}
export function deactivate() {}  // optional
```

| `api.` | |
|---|---|
| `id` | The plugin's id. |
| `registerTool(spec)` | Like a `tools` entry; `run(args)` may replace `commands`. |
| `registerCommand(name, handler)` | Handler for `{ type: "plugin.run", plugin, command, args }`, usable from declarative tools too. |
| `execute(command)` | Runs any Command Bus command. |
| `on(type, handler)` | Events: `pane.opened`, `pane.closed`, `pane.status` (`status`, `previous`: idle / working / needs-input / done / error). |
| `state()` | Snapshot of `workspaces` (with panes), `activeWorkspaceId`, `focusedPaneId`. |
| `describePane(paneId)` | `{ paneId, workspaceId, workspaceName, cwd, provider, label }` like an agent tool's `caller`, or null. |
| `readPane(paneId, lines?)` | The last `lines` (default 40) of a pane's terminal as plain text, or null. |
| `paneRequest(paneId)` | What an agent in `needs-input` asks: `{ kind: "approval" \| "question", text }`, or null. |
| `toast(text, kind?)` | `info`, `success` or `error`. |
| `confirm(message, { title?, okLabel?, kind? })` | Asks the user; resolves to true if they agreed. |
| `registerPage({ id, title, icon?, mount(el) })` | A tab in the title bar next to Overview. `mount` renders into `el` and may return a cleanup. Icons: bot, database, file, globe, key, lock, puzzle, shield, star, terminal. |
| `openPage(id)` | Shows one of the plugin's pages. |
| `registerAgentTool({ name, description, inputSchema?, run(args, caller) })` | A tool agents started by winfer can call (served by winfer's MCP server). `caller` has `paneId`, `workspaceId`, `workspaceName`, `cwd`, `provider` and `label`; return the text the agent sees, or throw. |
| `vault` | Secrets in the OS keychain, per plugin (below). |

Each `register*` and `on` returns a function that undoes it; everything is undone when the plugin
is turned off or reloaded. The module is a single file: bundle dependencies into it.

### Vault

Values are stored in the keychain and stay in winfer's Rust side; only names and the metadata you
pass are on disk. Keys look like environment variables (`API_TOKEN`).

| `api.vault.` | |
|---|---|
| `list()` | `[{ key, meta, updatedAt }]` |
| `set(key, value \| undefined, meta?)` | Saves `meta`, and the value unless it's `undefined`. |
| `delete(key)` | |
| `reveal(key)` | The value, to show the user. Never give it to an agent. |
| `exec({ command, cwd, keys, timeoutSec? })` | Runs a shell command with those secrets as environment variables; `{ code, timedOut, output }` with the values (also base64/URL-encoded) replaced by `[secret KEY]`. |
| `fill({ workspaceId, key, selector, tab? })` | Types a secret into a field of the workspace's browser tab; resolves to the tab's URL. |

### Page styles

Pages can use the app's look through these classes (they follow light/dark mode): `wf-page`,
`wf-header`, `wf-h1`, `wf-h2`, `wf-muted`, `wf-subtle`, `wf-mono`, `wf-card`, `wf-row` (with
`wf-grow`), `wf-stack`, `wf-inline`, `wf-field`, `wf-input`, `wf-select`, `wf-button` (+
`wf-primary`, `wf-danger`, `wf-ghost`), `wf-badge` (+ `wf-accent`), `wf-switch` (a checkbox),
`wf-empty`, `wf-error`, `wf-dialog-backdrop` and `wf-dialog`. Colors are CSS variables such as
`var(--color-fg)` and `var(--color-accent)`.
