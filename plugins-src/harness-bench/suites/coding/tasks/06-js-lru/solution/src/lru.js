export class LRUCache {
  #map = new Map(); // insertion order = recency, oldest first
  #max;
  #ttl;
  #now;
  #onEvict;

  constructor({ max, ttl = 0, now = Date.now, onEvict } = {}) {
    if (!Number.isInteger(max) || max < 1) throw new RangeError("max must be an integer >= 1");
    this.#max = max;
    this.#ttl = ttl;
    this.#now = now;
    this.#onEvict = onEvict;
  }

  #expired(e) {
    return e.expires !== 0 && this.#now() >= e.expires;
  }

  #drop(key, e, reason) {
    this.#map.delete(key);
    this.#onEvict?.(key, e.value, reason);
  }

  #live(key) {
    const e = this.#map.get(key);
    if (!e) return undefined;
    if (this.#expired(e)) {
      this.#drop(key, e, "expire");
      return undefined;
    }
    return e;
  }

  #purge() {
    for (const [k, e] of this.#map) if (this.#expired(e)) this.#drop(k, e, "expire");
  }

  set(key, value, { ttl = this.#ttl } = {}) {
    this.#map.delete(key);
    if (this.#map.size >= this.#max) this.#purge();
    while (this.#map.size >= this.#max) {
      const [k, e] = this.#map.entries().next().value;
      this.#drop(k, e, "evict");
    }
    this.#map.set(key, { value, expires: ttl ? this.#now() + ttl : 0 });
    return this;
  }

  get(key) {
    const e = this.#live(key);
    if (!e) return undefined;
    this.#map.delete(key);
    this.#map.set(key, e);
    return e.value;
  }

  peek(key) {
    return this.#live(key)?.value;
  }

  has(key) {
    return this.#live(key) !== undefined;
  }

  delete(key) {
    const e = this.#map.get(key);
    if (!e) return false;
    this.#map.delete(key);
    return !this.#expired(e);
  }

  clear() {
    this.#map.clear();
  }

  get size() {
    this.#purge();
    return this.#map.size;
  }

  keys() {
    this.#purge();
    return [...this.#map.keys()].reverse();
  }
}
