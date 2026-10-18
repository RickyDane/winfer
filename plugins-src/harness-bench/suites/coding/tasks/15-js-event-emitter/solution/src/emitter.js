export class Emitter {
  constructor({ maxListeners = 10, warn = (m) => console.warn(m) } = {}) {
    this._map = new Map(); // name -> [{ fn, once }]
    this._max = maxListeners;
    this._warn = warn;
    this._warned = new Set();
  }

  _add(name, fn, once) {
    if (typeof fn !== "function") throw new TypeError("listener must be a function");
    let list = this._map.get(name);
    if (!list) this._map.set(name, (list = []));
    list.push({ fn, once });
    if (this._max > 0 && list.length > this._max && !this._warned.has(name)) {
      this._warned.add(name);
      this._warn(`Possible memory leak: ${list.length} listeners for event "${String(name)}" (max ${this._max})`);
    }
    return this;
  }

  on(name, fn) { return this._add(name, fn, false); }
  once(name, fn) { return this._add(name, fn, true); }

  off(name, fn) {
    if (arguments.length === 0) { this._map.clear(); return this; }
    const list = this._map.get(name);
    if (!list) return this;
    if (fn === undefined) { this._map.delete(name); return this; }
    for (let i = list.length - 1; i >= 0; i--) {
      if (list[i].fn === fn) { list.splice(i, 1); break; }
    }
    if (list.length === 0) this._map.delete(name);
    return this;
  }

  _removeEntry(name, entry) {
    const list = this._map.get(name);
    if (!list) return;
    const i = list.indexOf(entry);
    if (i >= 0) list.splice(i, 1);
    if (list.length === 0) this._map.delete(name);
  }

  emit(name, ...args) {
    const named = (this._map.get(name) || []).slice();
    const wild = name === "*" ? [] : (this._map.get("*") || []).slice();
    if (name === "error" && named.length === 0) {
      const a = args[0];
      throw a instanceof Error ? a : new Error(`Unhandled error: ${String(a)}`);
    }
    const run = (n, entry, callArgs) => {
      if (entry.once) {
        if (entry.fired) return;
        entry.fired = true;
        this._removeEntry(n, entry);
      }
      entry.fn.apply(this, callArgs);
    };
    for (const e of named) run(name, e, name === "*" ? [name, ...args] : args);
    for (const e of wild) run("*", e, [name, ...args]);
    return named.length + wild.length > 0;
  }

  listenerCount(name) { return (this._map.get(name) || []).length; }
  listeners(name) { return (this._map.get(name) || []).map((e) => e.fn); }
  eventNames() { return [...this._map.keys()]; }
}
