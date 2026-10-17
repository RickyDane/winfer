// RFC 6902 JSON Patch. See README.md.

export class PatchError extends Error {}

const clone = (v) => (v === undefined ? v : JSON.parse(JSON.stringify(v)));
const isObj = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const has = (o, k) => Object.prototype.hasOwnProperty.call(o, k);

function parse(ptr) {
  if (typeof ptr !== "string") throw new PatchError("pointer must be a string");
  if (ptr === "") return [];
  if (ptr[0] !== "/") throw new PatchError(`bad pointer ${ptr}`);
  return ptr
    .slice(1)
    .split("/")
    .map((t) => {
      if (/~[^01]|~$/.test(t)) throw new PatchError(`bad escape in ${ptr}`);
      return t.replace(/~1/g, "/").replace(/~0/g, "~");
    });
}

function index(arr, tok, forAdd) {
  if (forAdd && tok === "-") return arr.length;
  if (!/^(0|[1-9][0-9]*)$/.test(tok)) throw new PatchError(`bad array index ${tok}`);
  const i = Number(tok);
  if (i > arr.length || (!forAdd && i === arr.length)) throw new PatchError(`index ${tok} out of range`);
  return i;
}

function get(doc, tokens) {
  let cur = doc;
  for (const t of tokens) {
    if (Array.isArray(cur)) cur = cur[index(cur, t, false)];
    else if (isObj(cur) && has(cur, t)) cur = cur[t];
    else throw new PatchError(`path not found: /${tokens.join("/")}`);
  }
  return cur;
}

function equal(a, b) {
  if (a === b) return true;
  if (Array.isArray(a)) return Array.isArray(b) && a.length === b.length && a.every((x, i) => equal(x, b[i]));
  if (isObj(a) && isObj(b)) {
    const ka = Object.keys(a);
    return ka.length === Object.keys(b).length && ka.every((k) => has(b, k) && equal(a[k], b[k]));
  }
  return false;
}

// Each returns the new root.
function add(doc, tokens, value) {
  if (!tokens.length) return value;
  const parent = get(doc, tokens.slice(0, -1));
  const last = tokens[tokens.length - 1];
  if (Array.isArray(parent)) parent.splice(index(parent, last, true), 0, value);
  else if (isObj(parent)) Object.defineProperty(parent, last, { value, writable: true, enumerable: true, configurable: true });
  else throw new PatchError("parent is not a container");
  return doc;
}

function remove(doc, tokens) {
  if (!tokens.length) throw new PatchError("cannot remove the root");
  const parent = get(doc, tokens.slice(0, -1));
  const last = tokens[tokens.length - 1];
  if (Array.isArray(parent)) parent.splice(index(parent, last, false), 1);
  else if (isObj(parent) && has(parent, last)) delete parent[last];
  else throw new PatchError(`path not found: /${tokens.join("/")}`);
  return doc;
}

export function applyPatch(doc, patch) {
  if (!Array.isArray(patch)) throw new PatchError("patch must be an array");
  let out = clone(doc);
  for (const op of patch) {
    if (!isObj(op)) throw new PatchError("operation must be an object");
    const path = parse(op.path);
    const need = (k) => {
      if (!has(op, k)) throw new PatchError(`${op.op} needs ${k}`);
      return op[k];
    };
    switch (op.op) {
      case "add":
        out = add(out, path, clone(need("value")));
        break;
      case "remove":
        out = remove(out, path);
        break;
      case "replace": {
        const value = clone(need("value"));
        get(out, path);
        out = path.length ? add(remove(out, path), path, value) : value;
        break;
      }
      case "move": {
        const from = parse(need("from"));
        if (op.from === op.path) {
          get(out, from);
          break;
        }
        if (op.path.startsWith(op.from + "/")) throw new PatchError("cannot move into own child");
        const value = get(out, from);
        out = add(remove(out, from), path, value);
        break;
      }
      case "copy":
        out = add(out, path, clone(get(out, parse(need("from")))));
        break;
      case "test":
        if (!equal(get(out, path), need("value"))) throw new PatchError(`test failed at ${op.path}`);
        break;
      default:
        throw new PatchError(`unknown op ${op.op}`);
    }
  }
  return out;
}
