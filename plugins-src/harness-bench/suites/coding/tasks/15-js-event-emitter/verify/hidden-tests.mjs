import { test } from "node:test";
import assert from "node:assert/strict";
import { Emitter } from "../src/emitter.js";

const quiet = () => new Emitter({ warn: () => {} });

test("on/emit order, args, this, return value, chaining", () => {
  const e = quiet();
  const calls = [];
  let self;
  const r = e.on("x", function (a, b) { self = this; calls.push(["1", a, b]); })
    .on("x", (a) => calls.push(["2", a]));
  assert.equal(r, e);
  assert.equal(e.emit("x", 1, 2), true);
  assert.equal(self, e);
  assert.deepEqual(calls, [["1", 1, 2], ["2", 1]]);
  assert.equal(e.emit("nothing"), false);
  assert.throws(() => e.on("x", 42), TypeError);
});

test("duplicates and off removes most recent registration", () => {
  const e = quiet();
  const log = [];
  const f = (t) => log.push("f" + t);
  const g = () => log.push("g");
  e.on("a", f).on("a", g).on("a", f);
  assert.equal(e.listenerCount("a"), 3);
  e.emit("a", 1);
  assert.deepEqual(log, ["f1", "g", "f1"]);
  e.off("a", f);
  assert.deepEqual(e.listeners("a"), [f, g]);
  e.off("a", () => {});
  assert.equal(e.listenerCount("a"), 2);
  assert.equal(e.off("a", f), e);
  assert.deepEqual(e.listeners("a"), [g]);
});

test("off(name) and off()", () => {
  const e = quiet();
  e.on("a", () => {}).on("a", () => {}).on("b", () => {});
  e.off("a");
  assert.equal(e.listenerCount("a"), 0);
  assert.deepEqual(e.eventNames(), ["b"]);
  e.on("c", () => {});
  e.off();
  assert.deepEqual(e.eventNames(), []);
  assert.equal(e.emit("b"), false);
});

test("once runs once, even when re-emitting from inside", () => {
  const e = quiet();
  let n = 0;
  const f = () => { n++; e.emit("o"); };
  e.once("o", f);
  assert.deepEqual(e.listeners("o"), [f]);
  assert.equal(e.emit("o"), true);
  assert.equal(n, 1);
  assert.equal(e.emit("o"), false);
  assert.equal(e.listenerCount("o"), 0);
});

test("off with original function removes once listener", () => {
  const e = quiet();
  let n = 0;
  const f = () => n++;
  e.once("o", f);
  e.off("o", f);
  assert.equal(e.emit("o"), false);
  assert.equal(n, 0);
});

test("once and on mixed keep order", () => {
  const e = quiet();
  const log = [];
  e.on("m", () => log.push(1)).once("m", () => log.push(2)).on("m", () => log.push(3));
  e.emit("m");
  e.emit("m");
  assert.deepEqual(log, [1, 2, 3, 1, 3]);
});

test("snapshot: added during emit not called, removed during emit still called", () => {
  const e = quiet();
  const log = [];
  const late = () => log.push("late");
  const b = () => log.push("b");
  e.on("s", () => { log.push("a"); e.on("s", late); e.off("s", b); });
  e.on("s", b);
  e.emit("s");
  assert.deepEqual(log, ["a", "b"]);
  log.length = 0;
  e.emit("s");
  assert.deepEqual(log, ["a", "late"]);
});

test("wildcard after named, gets name; emit('*') calls once", () => {
  const e = quiet();
  const log = [];
  e.on("*", (...a) => log.push(["*", ...a]));
  e.on("x", (v) => log.push(["x", v]));
  assert.equal(e.emit("x", 5), true);
  assert.equal(e.emit("y", 6, 7), true);
  assert.deepEqual(log, [["x", 5], ["*", "x", 5], ["*", "y", 6, 7]]);
  log.length = 0;
  e.emit("*", 1);
  assert.deepEqual(log, [["*", "*", 1]]);
  assert.equal(e.listenerCount("x"), 1);
  assert.equal(e.listenerCount("*"), 1);
});

test("wildcard snapshot taken at start", () => {
  const e = quiet();
  const log = [];
  e.on("x", () => e.on("*", () => log.push("w")));
  e.emit("x");
  assert.deepEqual(log, []);
  e.emit("z");
  assert.deepEqual(log, ["w"]);
});

test("error event semantics", () => {
  const e = quiet();
  const boom = new Error("boom");
  assert.throws(() => e.emit("error", boom), (err) => err === boom);
  assert.throws(() => e.emit("error", "bad thing"), (err) => err instanceof Error && err.message.includes("bad thing"));
  assert.throws(() => e.emit("error"), Error);
  let wild = 0;
  e.on("*", () => wild++);
  assert.throws(() => e.emit("error", boom));
  assert.equal(wild, 0);
  const got = [];
  e.on("error", (x) => got.push(x));
  assert.equal(e.emit("error", boom), true);
  assert.deepEqual(got, [boom]);
  assert.equal(wild, 1);
});

test("throwing listener stops the emit", () => {
  const e = quiet();
  const log = [];
  e.on("t", () => { throw new RangeError("x"); });
  e.on("t", () => log.push("after"));
  e.on("*", () => log.push("wild"));
  assert.throws(() => e.emit("t"), RangeError);
  assert.deepEqual(log, []);
});

test("max listeners warning once per name", () => {
  const msgs = [];
  const e = new Emitter({ maxListeners: 2, warn: (m) => msgs.push(m) });
  e.on("a", () => {}).on("a", () => {});
  assert.equal(msgs.length, 0);
  e.on("a", () => {});
  assert.equal(msgs.length, 1);
  assert.ok(String(msgs[0]).includes("a"));
  e.off("a");
  e.on("a", () => {}).on("a", () => {}).on("a", () => {}).on("a", () => {});
  assert.equal(msgs.length, 1);
  e.on("zebra", () => {}).on("zebra", () => {}).once("zebra", () => {});
  assert.equal(msgs.length, 2);
  assert.ok(String(msgs[1]).includes("zebra"));
});

test("maxListeners 0 is unlimited; default is 10", () => {
  const msgs = [];
  const e = new Emitter({ maxListeners: 0, warn: (m) => msgs.push(m) });
  for (let i = 0; i < 50; i++) e.on("a", () => {});
  assert.equal(msgs.length, 0);
  const d = new Emitter({ warn: (m) => msgs.push(m) });
  for (let i = 0; i < 10; i++) d.on("b", () => {});
  assert.equal(msgs.length, 0);
  d.on("b", () => {});
  assert.equal(msgs.length, 1);
});

test("eventNames order, listeners returns copy", () => {
  const e = quiet();
  const f = () => {};
  e.on("b", f).on("a", f).on("b", f);
  assert.deepEqual(e.eventNames(), ["b", "a"]);
  e.off("b");
  e.on("b", f);
  assert.deepEqual(e.eventNames(), ["a", "b"]);
  const ls = e.listeners("a");
  ls.push(() => {});
  assert.equal(e.listenerCount("a"), 1);
  assert.deepEqual(e.listeners("none"), []);
});
