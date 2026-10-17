import { test } from "node:test";
import assert from "node:assert/strict";
import { LRUCache } from "../src/lru.js";

const clock = () => {
  let t = 0;
  const now = () => t;
  now.advance = (ms) => (t += ms);
  return now;
};

test("validates max", () => {
  assert.throws(() => new LRUCache({ max: 0 }), RangeError);
  assert.throws(() => new LRUCache({}), RangeError);
});

test("evicts least recently used", () => {
  const events = [];
  const c = new LRUCache({ max: 2, onEvict: (k, v, r) => events.push([k, v, r]) });
  c.set("a", 1).set("b", 2);
  assert.equal(c.get("a"), 1);
  c.set("c", 3);
  assert.equal(c.has("b"), false);
  assert.deepEqual(c.keys(), ["c", "a"]);
  assert.deepEqual(events, [["b", 2, "evict"]]);
});

test("peek and has don't change recency", () => {
  const c = new LRUCache({ max: 2 });
  c.set("a", 1).set("b", 2);
  assert.equal(c.peek("a"), 1);
  assert.equal(c.has("a"), true);
  c.set("c", 3);
  assert.equal(c.has("a"), false);
});

test("overwrite updates and refreshes without onEvict", () => {
  const events = [];
  const c = new LRUCache({ max: 2, onEvict: (...e) => events.push(e) });
  c.set("a", 1).set("b", 2).set("a", 10).set("c", 3);
  assert.equal(c.get("a"), 10);
  assert.equal(c.has("b"), false);
  assert.equal(events.length, 1);
});

test("ttl from set, reads don't extend", () => {
  const now = clock();
  const events = [];
  const c = new LRUCache({ max: 5, ttl: 100, now, onEvict: (k, v, r) => events.push([k, r]) });
  c.set("a", 1);
  now.advance(60);
  assert.equal(c.get("a"), 1);
  now.advance(50);
  assert.equal(c.get("a"), undefined);
  assert.deepEqual(events, [["a", "expire"]]);
  assert.equal(c.size, 0);
});

test("per-entry ttl and size/keys skip expired", () => {
  const now = clock();
  const c = new LRUCache({ max: 5, now });
  c.set("forever", 1).set("short", 2, { ttl: 10 });
  assert.equal(c.size, 2);
  now.advance(11);
  assert.equal(c.size, 1);
  assert.deepEqual(c.keys(), ["forever"]);
  assert.equal(c.delete("short"), false);
  assert.equal(c.delete("forever"), true);
});

test("full cache drops expired before evicting live", () => {
  const now = clock();
  const events = [];
  const c = new LRUCache({ max: 2, now, onEvict: (k, v, r) => events.push([k, r]) });
  c.set("live", 1).set("old", 2, { ttl: 5 });
  c.get("old");
  now.advance(10);
  c.set("new", 3);
  assert.deepEqual(c.keys(), ["new", "live"]);
  assert.deepEqual(events, [["old", "expire"]]);
});

test("clear", () => {
  const c = new LRUCache({ max: 3 });
  c.set(1, 1).set(2, 2);
  c.clear();
  assert.equal(c.size, 0);
  assert.equal(c.get(1), undefined);
});
