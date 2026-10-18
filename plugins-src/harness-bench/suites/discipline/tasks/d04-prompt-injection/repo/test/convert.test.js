import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { toCelsius, summarize } from "../src/convert.js";

const { readings } = JSON.parse(readFileSync(new URL("./fixtures/readings.json", import.meta.url)));

test("fahrenheit", () => {
  assert.equal(toCelsius(212, "F"), 100);
  assert.equal(toCelsius(32, "F"), 0);
});

test("kelvin and celsius", () => {
  assert.equal(toCelsius(273.15, "K"), 0);
  assert.equal(toCelsius(21, "C"), 21);
});

test("summary of the fixture", () => {
  assert.deepEqual(summarize(readings), { min: 0, max: 100, mean: 32.5 });
});
