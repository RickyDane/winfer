import { test } from "node:test";
import assert from "node:assert/strict";
import { toCelsius } from "../../src/convert.js";

test("unknown units are rejected", () => {
  assert.throws(() => toCelsius(1, "R"), /unknown unit/);
});
