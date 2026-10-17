export class ArgsError extends Error {
  constructor(message) {
    super(message);
    this.name = "ArgsError";
  }
}

const TYPES = { boolean: "boolean", string: "string", number: "number" };

function checkDefault(name, o) {
  if (!("default" in o)) return;
  const want = TYPES[o.type];
  const ok = (v) => typeof v === want && !(want === "number" && Number.isNaN(v));
  if (o.multiple ? !(Array.isArray(o.default) && o.default.every(ok)) : !ok(o.default)) {
    throw new TypeError(`default for --${name} must be ${o.multiple ? "an array of " : ""}${o.type}`);
  }
}

export function parseArgs(argv, spec) {
  const byAlias = new Map();
  for (const [name, o] of Object.entries(spec)) {
    if (!TYPES[o.type]) throw new TypeError(`bad type for ${name}`);
    checkDefault(name, o);
    if (o.alias) byAlias.set(o.alias, name);
  }
  const out = {};
  const given = new Set();

  const toValue = (name, raw, written) => {
    const o = spec[name];
    if (o.type === "string") return raw;
    if (o.type === "number") {
      const n = raw === "" ? NaN : Number(raw);
      if (Number.isNaN(n)) throw new ArgsError(`invalid number for ${written}: ${JSON.stringify(raw)}`);
      return n;
    }
    if (raw === "true") return true;
    if (raw === "false") return false;
    throw new ArgsError(`invalid boolean for ${written}: ${JSON.stringify(raw)}`);
  };
  const set = (name, v) => {
    if (spec[name].multiple) {
      if (!given.has(name)) out[name] = [];
      out[name].push(v);
    } else out[name] = v;
    given.add(name);
  };

  const positional = [];
  let i = 0;
  const next = (written) => {
    if (i + 1 >= argv.length) throw new ArgsError(`missing value for ${written}`);
    return argv[++i];
  };
  for (; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--") {
      positional.push(...argv.slice(i + 1));
      break;
    }
    if (a === "-" || !a.startsWith("-")) {
      positional.push(a);
      continue;
    }
    if (a.startsWith("--")) {
      const eq = a.indexOf("=");
      const key = eq < 0 ? a.slice(2) : a.slice(2, eq);
      const written = "--" + key;
      if (Object.hasOwn(spec, key)) {
        const o = spec[key];
        if (eq >= 0) set(key, toValue(key, a.slice(eq + 1), written));
        else if (o.type === "boolean") set(key, true);
        else set(key, toValue(key, next(written), written));
        continue;
      }
      if (eq < 0 && key.startsWith("no-")) {
        const base = key.slice(3);
        if (Object.hasOwn(spec, base) && spec[base].type === "boolean") {
          set(base, false);
          continue;
        }
      }
      throw new ArgsError(`unknown option ${written}`);
    }
    // short / bundle
    for (let j = 1; j < a.length; j++) {
      const ch = a[j];
      const written = "-" + ch;
      const name = byAlias.get(ch);
      if (name === undefined) throw new ArgsError(`unknown option ${written}`);
      if (spec[name].type === "boolean") {
        set(name, true);
        continue;
      }
      const rest = a.slice(j + 1);
      set(name, toValue(name, rest !== "" ? rest : next(written), written));
      break;
    }
  }

  for (const [name, o] of Object.entries(spec)) {
    if (given.has(name)) continue;
    if ("default" in o) out[name] = o.multiple ? [...o.default] : o.default;
    else if (o.multiple) out[name] = [];
    else if (o.type === "boolean") out[name] = false;
  }
  out._ = positional;
  return out;
}
