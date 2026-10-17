import { test } from "node:test";
import assert from "node:assert/strict";
import { parse, format, overlaps, merge, gaps, totalMs, splitByDay } from "../src/ranges.js";

const R = (s) => parse(s);
const F = (rs) => rs.map(format);
const iso = (s) => new Date(s).toISOString();

test("overlaps edge cases", () => {
  const a = R("2024-01-01T10:00Z/2024-01-01T12:00Z");
  assert.equal(overlaps(a, R("2024-01-01T08:00Z/2024-01-01T10:00Z")), false);
  assert.equal(overlaps(R("2024-01-01T08:00Z/2024-01-01T10:00Z"), a), false);
  assert.equal(overlaps(a, R("2024-01-01T11:59:59.999Z/2024-01-01T13:00Z")), true);
  assert.equal(overlaps(a, R("2024-01-01T10:30Z/2024-01-01T11:00Z")), true);
  assert.equal(overlaps(R("2024-01-01T09:00Z/2024-01-01T13:00Z"), a), true);
  assert.equal(overlaps(a, R("2024-01-01T13:00Z/2024-01-01T14:00Z")), false);
});

test("merge doesn't modify its input", () => {
  const input = [R("2024-01-01T09:00Z/2024-01-01T11:00Z"), R("2024-01-01T10:00Z/2024-01-01T12:00Z"), R("2024-01-01T08:00Z/2024-01-01T08:30Z")];
  const before = F(input);
  const got = merge(input);
  assert.deepEqual(F(input), before, "input ranges changed");
  assert.deepEqual(F(got), ["2024-01-01T08:00:00.000Z/2024-01-01T08:30:00.000Z", "2024-01-01T09:00:00.000Z/2024-01-01T12:00:00.000Z"]);
  got[0].end = new Date(0);
  assert.deepEqual(F(input), before, "result shares objects with input");
});

test("merge chains, containment, empties, unsorted", () => {
  const got = merge([
    R("2024-01-01T15:00Z/2024-01-01T16:00Z"),
    R("2024-01-01T10:00Z/2024-01-01T14:00Z"),
    R("2024-01-01T11:00Z/2024-01-01T12:00Z"),
    R("2024-01-01T12:30Z/2024-01-01T13:00Z"),
    R("2024-01-01T14:00Z/2024-01-01T14:30Z"),
    R("2024-01-01T20:00Z/2024-01-01T20:00Z"),
    R("2024-01-01T09:00Z/2024-01-01T10:00Z"),
  ]);
  assert.deepEqual(F(got), [
    "2024-01-01T09:00:00.000Z/2024-01-01T14:30:00.000Z",
    "2024-01-01T15:00:00.000Z/2024-01-01T16:00:00.000Z",
  ]);
  assert.deepEqual(merge([]), []);
});

test("totalMs with containment", () => {
  assert.equal(totalMs([R("2024-01-01T00:00Z/2024-01-02T00:00Z"), R("2024-01-01T05:00Z/2024-01-01T06:00Z"), R("2024-01-01T23:00Z/2024-01-02T01:00Z")]), 25 * 3600e3);
});

test("gaps with containment and edges", () => {
  const within = R("2024-01-01T00:00Z/2024-01-01T10:00Z");
  const ranges = [R("2023-12-31T20:00Z/2024-01-01T02:00Z"), R("2024-01-01T03:00Z/2024-01-01T08:00Z"), R("2024-01-01T04:00Z/2024-01-01T05:00Z"), R("2024-01-01T09:00Z/2024-01-01T12:00Z")];
  const before = F(ranges);
  assert.deepEqual(F(gaps(ranges, within)), ["2024-01-01T02:00:00.000Z/2024-01-01T03:00:00.000Z", "2024-01-01T08:00:00.000Z/2024-01-01T09:00:00.000Z"]);
  assert.deepEqual(F(ranges), before);
  assert.deepEqual(F(gaps([], within)), [format(within)]);
});

test("splitByDay edge cases", () => {
  assert.deepEqual(splitByDay(R("2024-02-29T00:00Z/2024-02-29T00:00Z")), []);
  assert.deepEqual(F(splitByDay(R("2024-02-28T00:00Z/2024-03-01T00:00Z"))), [
    "2024-02-28T00:00:00.000Z/2024-02-29T00:00:00.000Z",
    "2024-02-29T00:00:00.000Z/2024-03-01T00:00:00.000Z",
  ]);
  assert.deepEqual(F(splitByDay(R("2024-12-31T23:59Z/2025-01-01T00:01Z"))), [
    "2024-12-31T23:59:00.000Z/2025-01-01T00:00:00.000Z",
    "2025-01-01T00:00:00.000Z/2025-01-01T00:01:00.000Z",
  ]);
  assert.deepEqual(F(splitByDay(R("2024-03-10T01:00Z/2024-03-10T09:00Z"))), ["2024-03-10T01:00:00.000Z/2024-03-10T09:00:00.000Z"]);
  const pieces = splitByDay(R("2024-03-30T12:00Z/2024-04-02T12:00Z"));
  assert.equal(pieces.length, 4);
  assert.equal(pieces.reduce((s, p) => s + (p.end - p.start), 0), 3 * 24 * 3600e3);
  assert.equal(iso(pieces[1].start), "2024-03-31T00:00:00.000Z");
});
