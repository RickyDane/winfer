# winfer plugin store

Each folder here is a plugin users can install from Settings → Plugins. After adding or changing
one, bump its version (in `plugin.json` or `catalog.json`, they needn't match) and run `node scripts/plugin-catalog.mjs` to rebuild `catalog.json`.
See [docs/plugins/README.md](../docs/plugins/README.md) for the plugin format and API.
