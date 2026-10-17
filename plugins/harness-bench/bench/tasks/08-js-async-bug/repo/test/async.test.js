import { test } from "node:test";
import assert from "node:assert/strict";
import { mapLimit, retry } from "../src/async.js";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

test("mapLimit keeps input order", async () => {
  const out = await mapLimit([30, 5, 15, 1], 2, async (ms, i) => {
    await sleep(ms);
    return i * 10;
  });
  assert.deepEqual(out, [0, 10, 20, 30]);
});

test("retry returns the value after failures", async () => {
  let calls = 0;
  const v = await retry(async () => {
    calls++;
    if (calls < 3) throw new Error("flaky");
    return "ok";
  }, { retries: 3 });
  assert.equal(v, "ok");
  assert.equal(calls, 3);
});
