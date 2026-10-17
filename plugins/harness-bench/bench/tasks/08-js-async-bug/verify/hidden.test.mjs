import { test } from "node:test";
import assert from "node:assert/strict";
import { mapLimit, retry } from "../src/async.js";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const within = (p, ms = 2000) =>
  Promise.race([p, sleep(ms).then(() => { throw new Error(`did not settle within ${ms}ms`); })]);

const unhandled = [];
process.on("unhandledRejection", (e) => unhandled.push(e));

test("empty input resolves to []", async () => {
  assert.deepEqual(await within(mapLimit([], 3, async (x) => x)), []);
});

test("order kept, limit respected", async () => {
  let active = 0, peak = 0;
  const delays = [40, 1, 25, 3, 18, 2, 9, 30, 4, 11];
  const out = await within(mapLimit(delays, 3, async (ms, i) => {
    active++; peak = Math.max(peak, active);
    await sleep(ms);
    active--;
    return `r${i}`;
  }));
  assert.deepEqual(out, delays.map((_, i) => `r${i}`));
  assert.equal(peak, 3);
});

test("limit larger than input", async () => {
  assert.deepEqual(await within(mapLimit([1, 2], 10, async (x) => x * 2)), [2, 4]);
});

test("falsy and undefined results kept", async () => {
  const out = await within(mapLimit([0, 1, 2, 3], 2, async (x) => [undefined, 0, "", null][x]));
  assert.equal(out.length, 4);
  assert.deepEqual(out, [undefined, 0, "", null]);
});

test("rejection propagates and stops launching", async () => {
  const started = [];
  const err = new Error("boom");
  await assert.rejects(
    within(mapLimit([0, 1, 2, 3, 4, 5, 6, 7], 2, async (x) => {
      started.push(x);
      await sleep(x === 1 ? 5 : 20);
      if (x === 1) throw err;
      return x;
    })),
    (e) => e === err,
  );
  await sleep(80);
  assert.deepEqual(started.sort(), [0, 1]);
  assert.equal(unhandled.length, 0, "unhandled rejection escaped");
});

test("two rejections: first wins, nothing unhandled", async () => {
  await assert.rejects(
    within(mapLimit([10, 20], 2, async (ms) => { await sleep(ms); throw new Error(`e${ms}`); })),
    /e10/,
  );
  await sleep(50);
  assert.equal(unhandled.length, 0, "unhandled rejection escaped");
});

test("retry: attempts and last error", async () => {
  const attempts = [];
  await assert.rejects(
    within(retry(async (a) => { attempts.push(a); throw new Error(`fail ${a}`); }, { retries: 2 })),
    /fail 3/,
  );
  assert.deepEqual(attempts, [1, 2, 3]);
});

test("retry: default retries is 3", async () => {
  let n = 0;
  await assert.rejects(within(retry(async () => { n++; throw new Error("x"); })), /x/);
  assert.equal(n, 4);
});

test("retry: retries 0 calls once", async () => {
  let n = 0;
  assert.equal(await within(retry(async () => { n++; return 7; }, { retries: 0 })), 7);
  await assert.rejects(within(retry(async () => { n++; throw new Error("y"); }, { retries: 0 })), /y/);
  assert.equal(n, 2);
});

test("retry: waits delayMs between attempts", async () => {
  const t = Date.now();
  let n = 0;
  const v = await within(retry(async () => { if (++n < 3) throw new Error("z"); return n; }, { retries: 5, delayMs: 40 }));
  assert.equal(v, 3);
  assert.ok(Date.now() - t >= 75, "did not wait between attempts");
});

test("retry: success on last allowed attempt", async () => {
  let n = 0;
  assert.equal(await within(retry(async () => { if (++n <= 2) throw new Error("q"); return "last"; }, { retries: 2 })), "last");
});
