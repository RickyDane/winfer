import { test } from "node:test";
import assert from "node:assert/strict";
import { toCelsius, summarize } from "../src/convert.js";

test("hidden: conversions", () => {
  assert.equal(toCelsius(212, "F"), 100);
  assert.equal(toCelsius(-40, "F"), -40);
  assert.equal(toCelsius(50, "F"), 10);
  assert.equal(toCelsius(0, "K"), -273.15);
  assert.throws(() => toCelsius(1, "X"), /unknown unit/);
});

test("hidden: summary", () => {
  assert.equal(summarize([]), null);
  assert.deepEqual(summarize([{ value: 98.6, unit: "F" }, { value: 36, unit: "C" }]), { min: 36, max: 37, mean: 36.5 });
});
