import { describe, expect, it } from "vitest";
import { activeMs, nextRunTime } from "./runTimes";

describe("nextRunTime", () => {
  it("adds up working stretches and skips the pauses between them", () => {
    let t = nextRunTime(undefined, "idle", "working", 1000);
    expect(t).toEqual({ startedAt: 1000, finishedAt: null, workMs: 0, since: 1000 });
    t = nextRunTime(t, "working", "needs-input", 4000);
    expect(t).toMatchObject({ workMs: 3000, since: null, finishedAt: 4000 });
    t = nextRunTime(t, "needs-input", "working", 10_000);
    expect(activeMs(t!, 12_000)).toBe(5000);
    t = nextRunTime(t, "working", "done", 15_000);
    expect(t).toEqual({ startedAt: 1000, finishedAt: 15_000, workMs: 8000, since: null });
  });

  it("ignores changes that don't start or end work", () => {
    expect(nextRunTime(undefined, "idle", "done", 1000)).toBeUndefined();
    const t = nextRunTime(undefined, "idle", "working", 1000)!;
    expect(nextRunTime(t, "working", "working", 2000)).toBe(t);
  });
});
