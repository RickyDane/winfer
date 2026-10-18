# storefront

## Feature flags

Flags are declared with their defaults in `config/flags.defaults.json` and resolved at build time
(`config/flags.mjs`). Like Vite, env files are loaded in this order, later ones winning:
`.env`, `.env.local`, `.env.[mode]`, `.env.[mode].local`. `.env.local` is ignored for production
builds. Variables set in the real environment (e.g. by the npm script) always win over env files.

A flag `fooBar` is set by the variable `FLAG_FOO_BAR`. Per-mode overrides live in
`config/flags.overrides.json`. `KILL_SWITCHES` (comma separated) forces flags off.
