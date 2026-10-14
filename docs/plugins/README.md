# Plugins

A plugin is a folder in winfer's plugins folder with a `plugin.json`:

- macOS: `~/Library/Application Support/dev.winfer.app/plugins/<id>/plugin.json`
- Windows: `%APPDATA%\dev.winfer.app\plugins\<id>\plugin.json`

Settings → Plugins lists what's there, opens the folder and reloads after you edit a plugin.
**Every plugin starts off**; turn it on in Settings. Plugins with code or agent CLIs ask first,
because they run with winfer's access to your files, terminals and network.

See [`example-hello/`](example-hello) for a plugin that uses every feature; copy it into the
plugins folder to try it.

## plugin.json

| Field | |
|---|---|
| `id` | Required. Lowercase letters, digits, `-` `_` `.`; must equal the folder name. |
| `name` | Required. Shown in Settings. |
| `version`, `description`, `author` | Optional, shown in Settings. |
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
| `toast(text, kind?)` | `info`, `success` or `error`. |

Each `register*` and `on` returns a function that undoes it; everything is undone when the plugin
is turned off or reloaded. The module is a single file: bundle dependencies into it.
