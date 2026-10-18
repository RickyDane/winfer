import { test } from "node:test";
import assert from "node:assert/strict";
import { applyPatch, PatchError } from "../src/patch.js";

const ok = (doc, patch, want) => assert.deepStrictEqual(applyPatch(doc, patch), want);
const bad = (doc, patch) => {
  const before = JSON.stringify(doc);
  assert.throws(() => applyPatch(doc, patch), PatchError, JSON.stringify(patch));
  assert.equal(JSON.stringify(doc), before, "input was modified by a failing patch");
};

test("add", () => {
  ok({ a: 1 }, [{ op: "add", path: "/b", value: 2 }], { a: 1, b: 2 });
  ok({ a: 1 }, [{ op: "add", path: "/a", value: [3] }], { a: [3] });
  ok([1, 2], [{ op: "add", path: "/1", value: 9 }], [1, 9, 2]);
  ok([1, 2], [{ op: "add", path: "/2", value: 9 }], [1, 2, 9]);
  ok([1, 2], [{ op: "add", path: "/-", value: 9 }], [1, 2, 9]);
  ok({ a: { b: [] } }, [{ op: "add", path: "/a/b/-", value: { c: null } }], { a: { b: [{ c: null }] } });
  ok({ a: 1 }, [{ op: "add", path: "", value: [1] }], [1]);
  ok({}, [{ op: "add", path: "/", value: 1 }], { "": 1 });
  ok({ a: 1 }, [{ op: "add", path: "/n", value: null }], { a: 1, n: null });
  bad([1, 2], [{ op: "add", path: "/3", value: 9 }]);
  bad([1, 2], [{ op: "add", path: "/01", value: 9 }]);
  bad([1, 2], [{ op: "add", path: "/-1", value: 9 }]);
  bad([1, 2], [{ op: "add", path: "/x", value: 9 }]);
  bad({ a: 1 }, [{ op: "add", path: "/b/c", value: 9 }]);
  bad({ a: 1 }, [{ op: "add", path: "/b" }]);
  bad({ a: "str" }, [{ op: "add", path: "/a/0", value: 9 }]);
});

test("pointer escaping", () => {
  ok({ "a/b": 1, "m~n": 2 }, [{ op: "replace", path: "/a~1b", value: 3 }], { "a/b": 3, "m~n": 2 });
  ok({ "m~n": 2 }, [{ op: "remove", path: "/m~0n" }], {});
  ok({}, [{ op: "add", path: "/~01", value: 1 }], { "~1": 1 });
  ok({ "~1": 5 }, [{ op: "test", path: "/~01", value: 5 }], { "~1": 5 });
  bad({ a: 1 }, [{ op: "add", path: "a", value: 1 }]);
  bad({ "a/b": 1 }, [{ op: "remove", path: "/a/b" }]);
});

test("remove and replace", () => {
  ok({ a: 1, b: 2 }, [{ op: "remove", path: "/a" }], { b: 2 });
  ok([1, 2, 3], [{ op: "remove", path: "/1" }], [1, 3]);
  ok([1, 2, 3], [{ op: "replace", path: "/1", value: 7 }], [1, 7, 3]);
  ok({ a: { b: 1 } }, [{ op: "replace", path: "/a/b", value: [1] }], { a: { b: [1] } });
  ok({ a: 1 }, [{ op: "replace", path: "", value: "x" }], "x");
  ok({ a: null }, [{ op: "replace", path: "/a", value: 0 }], { a: 0 });
  ok({ a: null }, [{ op: "remove", path: "/a" }], {});
  bad({ a: 1 }, [{ op: "remove", path: "/b" }]);
  bad([1], [{ op: "remove", path: "/1" }]);
  bad([1], [{ op: "remove", path: "/-" }]);
  bad({ a: 1 }, [{ op: "replace", path: "/b", value: 1 }]);
  bad([1], [{ op: "replace", path: "/1", value: 1 }]);
  bad({ a: 1 }, [{ op: "replace", path: "/a" }]);
  bad({}, [{ op: "remove", path: "/toString" }]);
});

test("move and copy", () => {
  ok({ a: { x: 1 }, b: {} }, [{ op: "move", from: "/a/x", path: "/b/y" }], { a: {}, b: { y: 1 } });
  ok([1, 2, 3, 4], [{ op: "move", from: "/0", path: "/3" }], [2, 3, 4, 1]);
  ok([1, 2, 3, 4], [{ op: "move", from: "/3", path: "/0" }], [4, 1, 2, 3]);
  ok({ a: 1 }, [{ op: "move", from: "/a", path: "/a" }], { a: 1 });
  ok({ a: { b: 1 } }, [{ op: "move", from: "/a", path: "/ab" }], { ab: { b: 1 } });
  ok({ a: [1, { b: 2 }] }, [{ op: "copy", from: "/a/1", path: "/c" }], { a: [1, { b: 2 }], c: { b: 2 } });
  ok({ a: 1 }, [{ op: "copy", from: "", path: "/b" }], { a: 1, b: { a: 1 } });
  const r = applyPatch({ a: { b: 1 } }, [{ op: "copy", from: "/a", path: "/c" }]);
  r.c.b = 99;
  assert.equal(r.a.b, 1, "copy must deep-copy");
  bad({ a: { b: 1 } }, [{ op: "move", from: "/a", path: "/a/b/c" }]);
  bad({ a: 1 }, [{ op: "move", from: "/x", path: "/y" }]);
  bad({ a: 1 }, [{ op: "copy", path: "/y" }]);
  bad({ a: 1 }, [{ op: "copy", from: "/x", path: "/y" }]);
});

test("test op", () => {
  ok({ a: { x: 1, y: [1, 2] } }, [{ op: "test", path: "/a", value: { y: [1, 2], x: 1 } }], { a: { x: 1, y: [1, 2] } });
  ok([null], [{ op: "test", path: "/0", value: null }], [null]);
  ok("s", [{ op: "test", path: "", value: "s" }], "s");
  bad({ a: 1 }, [{ op: "test", path: "/a", value: "1" }]);
  bad({ a: [1, 2] }, [{ op: "test", path: "/a", value: [2, 1] }]);
  bad({ a: {} }, [{ op: "test", path: "/a", value: [] }]);
  bad({ a: null }, [{ op: "test", path: "/a", value: {} }]);
  bad({ a: 1 }, [{ op: "test", path: "/b", value: 1 }]);
});

test("unknown op and malformed patches", () => {
  bad({ a: 1 }, [{ op: "frobnicate", path: "/a" }]);
  bad({ a: 1 }, [{ op: "add", value: 1 }]);
  bad({ a: 1 }, [{ path: "/a", value: 1 }]);
});

test("atomic and pure", () => {
  const doc = { list: [1, 2, 3], meta: { n: 1 } };
  const snapshot = JSON.stringify(doc);
  bad(doc, [
    { op: "add", path: "/list/-", value: 4 },
    { op: "replace", path: "/meta/n", value: 2 },
    { op: "remove", path: "/meta" },
    { op: "test", path: "/list/0", value: 42 },
  ]);
  const out = applyPatch(doc, [
    { op: "add", path: "/list/0", value: 0 },
    { op: "remove", path: "/meta/n" },
  ]);
  assert.equal(JSON.stringify(doc), snapshot, "input mutated on success");
  assert.deepStrictEqual(out, { list: [0, 1, 2, 3], meta: {} });
  const untouched = applyPatch(doc, [{ op: "add", path: "/x", value: 1 }]);
  assert.notEqual(untouched.list, doc.list, "result shares arrays with the input");
  untouched.meta.z = 1;
  assert.equal(doc.meta.z, undefined);
  const value = { deep: [1] };
  const r = applyPatch({}, [{ op: "add", path: "/v", value }]);
  r.v.deep.push(2);
  assert.deepStrictEqual(value, { deep: [1] }, "result shares objects with the patch value");
});

test("sequence", () => {
  ok({ foo: "bar" }, [
    { op: "add", path: "/baz", value: "qux" },
    { op: "test", path: "/baz", value: "qux" },
    { op: "move", from: "/foo", path: "/moved" },
    { op: "copy", from: "/moved", path: "/arr" },
    { op: "replace", path: "/arr", value: [] },
    { op: "add", path: "/arr/-", value: 1 },
    { op: "add", path: "/arr/0", value: 0 },
  ], { baz: "qux", moved: "bar", arr: [0, 1] });
});
