# args

`src/args.js` exports `parseArgs(argv, spec)` and `class ArgsError extends Error`.

```js
parseArgs(["-vn", "3", "--out=a.txt", "in.txt"], {
  verbose: { type: "boolean", alias: "v" },
  num: { type: "number", alias: "n", default: 1 },
  out: { type: "string" },
  include: { type: "string", alias: "I", multiple: true },
});
// => { verbose: true, num: 3, out: "a.txt", include: [], _: ["in.txt"] }
```

## Spec

`spec` maps option names to `{ type, alias?, default?, multiple? }`:

- `type`: `"boolean"`, `"string"` or `"number"`.
- `alias`: a single character used with one dash (`-v`).
- `multiple`: if true, the option may be given several times and its value is an array of
  the values in the order given.
- `default`: used when the option isn't given. It must match the type (for `multiple`, an
  array whose items match the type) — otherwise `parseArgs` throws a `TypeError` before
  looking at `argv`. When there's no default: booleans are `false`, `multiple` options are
  `[]`, and other options are left out of the result entirely (no key).

## Result

An object with one key per option name (never alias keys), plus `_`: the positional
arguments as strings, in order. A `multiple` default array must not be shared with the spec
(mutating the result must not change `spec`).

## Syntax

- `--name` sets a boolean option to `true`. `--no-name` sets it to `false` (only for boolean
  options; `--no-x` for a non-boolean `x`, or with no option `x`, is an unknown option).
- `--name=value` and `--name value` give a string/number option its value. In the second
  form, the next argument is always taken as the value, even if it starts with `-`
  (`--num -5` is -5). `--name=` gives the empty string.
- A boolean also accepts `--name=true` / `--name=false`; any other `=value` is an error.
- `-x` is the option with alias `x`; same rules as the long form (`-n 5`). `-x=value` is not
  special syntax.
- Bundles: `-abc` is `-a -b -c`. If a letter in a bundle is a string/number option, the rest
  of the bundle is its value (`-vofile.txt` = `-v -o file.txt`); if it is the last letter, it
  takes the next argument (`-vo file.txt`).
- `--` ends the options: everything after it is positional (including more `--`).
- `-` on its own is a positional argument. Any other argument starting with `-` is an
  option (so `-5` as a positional is an unknown option).
- Number values are converted with `Number()`. An empty string, or anything that converts to
  `NaN`, is an error.
- Giving a non-`multiple` option more than once: the last one wins.

## Errors

Throw an `ArgsError` (an `Error` subclass, `name` = `"ArgsError"`) for:

- an unknown option — the message contains the option as written, e.g. `--colr` or `-q`
  (for a bundle, the unknown letter as `-q`);
- a missing value (string/number option at the end of argv) — the message contains the option
  name as written;
- an invalid number or an invalid boolean value.
