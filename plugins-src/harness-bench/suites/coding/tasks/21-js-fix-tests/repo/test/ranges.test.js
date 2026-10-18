import { test } from "node:test";
import assert from "node:assert/strict";
import { parse, format, overlaps, merge, gaps, totalMs, splitByDay } from "../src/ranges.js";

const R = (s) => parse(s);
const F = (rs) => rs.map(format);

test("parse and format", () => {
  assert.equal(format(R("2024-03-01T10:00:00Z/2024-03-01T11:30:00Z")), "2024-03-01T10:00:00.000Z/2024-03-01T11:30:00.000Z");
  assert.throws(() => R("2024-03-01T10:00Z/nope"), RangeError);
});

test("overlaps", () => {
  assert.equal(overlaps(R("2024-01-01T10:00Z/2024-01-01T12:00Z"), R("2024-01-01T11:00Z/2024-01-01T13:00Z")), true);
  assert.equal(overlaps(R("2024-01-01T10:00Z/2024-01-01T12:00Z"), R("2024-01-01T12:00Z/2024-01-01T13:00Z")), false);
});

test("merge joins overlapping and touching", () => {
  const got = merge([R("2024-01-01T12:00Z/2024-01-01T13:00Z"), R("2024-01-01T09:00Z/2024-01-01T10:00Z"), R("2024-01-01T10:00Z/2024-01-01T11:00Z")]);
  assert.deepEqual(F(got), ["2024-01-01T09:00:00.000Z/2024-01-01T11:00:00.000Z", "2024-01-01T12:00:00.000Z/2024-01-01T13:00:00.000Z"]);
});

test("merge keeps a contained range's outer end", () => {
  const got = merge([R("2024-01-01T08:00Z/2024-01-01T18:00Z"), R("2024-01-01T09:00Z/2024-01-01T10:00Z")]);
  assert.deepEqual(F(got), ["2024-01-01T08:00:00.000Z/2024-01-01T18:00:00.000Z"]);
});

test("gaps", () => {
  const got = gaps([R("2024-01-01T10:00Z/2024-01-01T11:00Z")], R("2024-01-01T09:00Z/2024-01-01T12:00Z"));
  assert.deepEqual(F(got), ["2024-01-01T09:00:00.000Z/2024-01-01T10:00:00.000Z", "2024-01-01T11:00:00.000Z/2024-01-01T12:00:00.000Z"]);
});

test("splitByDay", () => {
  assert.deepEqual(F(splitByDay(R("2024-01-01T22:00Z/2024-01-03T02:00Z"))), [
    "2024-01-01T22:00:00.000Z/2024-01-02T00:00:00.000Z",
    "2024-01-02T00:00:00.000Z/2024-01-03T00:00:00.000Z",
    "2024-01-03T00:00:00.000Z/2024-01-03T02:00:00.000Z",
  ]);
  assert.deepEqual(splitByDay(R("2024-01-01T05:00Z/2024-01-01T05:00Z")), []);
});

test("totalMs counts overlap once", () => {
  assert.equal(totalMs([R("2024-01-01T10:00Z/2024-01-01T12:00Z"), R("2024-01-01T11:00Z/2024-01-01T13:00Z")]), 3 * 3600e3);
});
