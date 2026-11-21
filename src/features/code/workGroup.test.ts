import { describe, expect, it } from "vitest";
import type { Item } from "./store";
import { buildDisplayRows, describeStep, fmtWorkedFor, groupDurationMs, summarizeGroup } from "./workGroup";

const user = (key: string): Item => ({ kind: "user", key, text: "hi", at: 1 });
const text = (key: string, streaming = false): Item => ({ kind: "text", key, text: "hello", streaming });
const reason = (key: string, streaming = false, started = 1000, ended?: number): Extract<Item, { kind: "reasoning" }> => ({
  kind: "reasoning",
  key,
  text: "thinking",
  streaming,
  started,
  ended,
});
const tool = (key: string, ok?: boolean, started = 1000, ended?: number): Item => ({
  kind: "tool",
  key,
  name: "Bash",
  title: "ls",
  input: {},
  output: "",
  ok,
  started,
  ended,
});
const notice = (key: string): Item => ({ kind: "notice", key, text: "note", tone: "info" });
const worked = (key: string, ms: number): Item => ({ kind: "worked", key, ms });

describe("buildDisplayRows", () => {
  it("shows a Working row right after a user message while busy", () => {
    const rows = buildDisplayRows([user("u1")], true, true);
    expect(rows).toEqual([
      { kind: "item", index: 0, key: "u1" },
      { kind: "working", key: "cw-working" },
    ]);
  });

  it("compacts reasoning, tools, and intermediate text into one active work group while running", () => {
    const items: Item[] = [user("u1"), reason("r1", false, 1000, 2000), tool("t1", true, 2000, 3000), text("a1"), tool("t2", undefined, 3000)];
    const rows = buildDisplayRows(items, true, true);
    expect(rows).toEqual([
      { kind: "item", index: 0, key: "u1" },
      { kind: "work", key: "wg-r1", start: 1, end: 4, workedMs: null, active: true },
    ]);
  });

  it("settles the work group and absorbs Worked once final text arrives and turn completes", () => {
    const items: Item[] = [
      user("u1"),
      reason("r1", false, 1000, 2000),
      tool("t1", true, 2000, 4000),
      text("a1"),
      tool("t2", true, 4000, 8000),
      text("a2"),
      worked("w1", 92_000),
    ];
    const rows = buildDisplayRows(items, false, true);
    expect(rows).toEqual([
      { kind: "item", index: 0, key: "u1" },
      { kind: "work", key: "wg-r1", start: 1, end: 4, workedMs: 92_000, active: false },
      { kind: "item", index: 5, key: "a2" },
    ]);
  });

  it("keeps notices outside the collapsed work group", () => {
    const items: Item[] = [
      user("u1"),
      tool("t1", true, 1000, 2000),
      tool("t2", true, 2000, 3000),
      notice("n1"),
      text("a2"),
      worked("w1", 5000),
    ];
    const rows = buildDisplayRows(items, false, true);
    expect(rows).toEqual([
      { kind: "item", index: 0, key: "u1" },
      { kind: "work", key: "wg-t1", start: 1, end: 2, workedMs: 5000, active: false },
      { kind: "item", index: 3, key: "n1" },
      { kind: "item", index: 4, key: "a2" },
    ]);
  });

  it("keeps flat rows when compactTools is disabled", () => {
    const items: Item[] = [user("u1"), tool("t1", true, 1000, 2000), text("a1")];
    const rows = buildDisplayRows(items, false, false);
    expect(rows.map((r) => r.kind)).toEqual(["item", "item", "item"]);
  });
});

describe("groupDurationMs & fmtWorkedFor & summarizeGroup", () => {
  it("formats durations cleanly", () => {
    expect(fmtWorkedFor(400)).toBe("Worked for 1s");
    expect(fmtWorkedFor(12_000)).toBe("Worked for 12s");
    expect(fmtWorkedFor(92_000)).toBe("Worked for 1m 32s");
    expect(fmtWorkedFor(204_000)).toBe("Worked for 3m 24s");
    expect(fmtWorkedFor(3_720_000)).toBe("Worked for 1h 02m");
  });

  it("uses explicit workedMs first, then item timestamps, and returns null for restored items", () => {
    const live: Item[] = [tool("t1", true, 1000, 2500), tool("t2", false, 2500, 5000)];
    expect(groupDurationMs(live, 0, 1, 9000)).toBe(9000);
    expect(groupDurationMs(live, 0, 1, null)).toBe(4000);

    const restored: Item[] = [tool("t1", true, 0, 0)];
    expect(groupDurationMs(restored, 0, 0, null)).toBeNull();

    const summary = summarizeGroup(live, 0, 1);
    expect(summary.toolCount).toBe(2);
    expect(summary.failedCount).toBe(1);
    expect(summary.lastStep?.key).toBe("t2");
  });

  it("describes active steps as short subtle phrases", () => {
    expect(describeStep(null)).toBe("Working…");
    expect(describeStep(reason("r1", true))).toBe("Thinking…");
    expect(describeStep(reason("r1", false, 1000, 2000))).toBe("Working…");
    expect(
      describeStep({ kind: "tool", key: "t1", name: "Read", title: "src/features/code/Transcript.tsx", input: {}, output: "", started: 1 }),
    ).toBe("Reading Transcript.tsx");
    expect(
      describeStep({ kind: "tool", key: "t2", name: "Edit", title: "native/crates/winfer-app/src/ui/code_view.rs", input: {}, output: "", started: 1 }),
    ).toBe("Editing code_view.rs");
    expect(
      describeStep({ kind: "tool", key: "t3", name: "Bash", title: "cargo test", input: { description: "Run unit tests" }, output: "", started: 1 }),
    ).toBe("Run unit tests");
    expect(
      describeStep({ kind: "tool", key: "t4", name: "Bash", title: "cargo test -p winfer-app", input: {}, output: "", started: 1 }),
    ).toBe("Running cargo test -p winfer-app");
    expect(
      describeStep({ kind: "tool", key: "t5", name: "WebFetch", title: "https://docs.rs/gpui/latest", input: {}, output: "", started: 1 }),
    ).toBe("Fetching docs.rs");
    expect(
      describeStep({ kind: "tool", key: "t6", name: "Bash", title: "ls", input: { description: "list project files" }, output: "", started: 1 }),
    ).toBe("List project files");
    expect(
      describeStep({ kind: "tool", key: "t7", name: "winfer · bg", title: "start web-os static server", input: {}, output: "", started: 1 }),
    ).toBe("Start web-os static server");
  });
});
