import { describe, expect, it } from "vitest";
import type { Message } from "../protocol";
import { buildDisplayRows, describeStep, elapsed, groupDurationMs, workGroupSummary } from "./compactTools";

const user = (id: string): Message => ({ id, role: "user", text: "hi", ts: 1000 });
const assistant = (id: string): Message => ({ id, role: "assistant", text: "ok", ts: null });
const tool = (id: string, toolName = "Bash", text = "ls", extra: Partial<Message> = {}): Message => ({
  id,
  role: "tool",
  tool: toolName,
  text,
  ts: 1000,
  ended: 2000,
  state: "done",
  ...extra,
});

describe("buildDisplayRows (compact tools)", () => {
  it("groups multi-tool runs into work groups and leaves single finished tools direct", () => {
    const msgs = [
      user("u1"),
      tool("t1", "Read", "src/a.ts", { ts: 1000, ended: 2000 }),
      tool("t2", "Edit", "src/a.ts", { ts: 2000, ended: 4000 }),
      assistant("a1"),
      tool("t3", "Bash", "pnpm test", { ts: 4000, ended: 8000 }),
      assistant("a2"),
    ];
    const { rows, hasLiveGroup } = buildDisplayRows(msgs, false, false, true);
    expect(hasLiveGroup).toBe(false);
    expect(rows.map((r) => r.kind)).toEqual(["msg", "work-group", "msg", "tools", "msg"]);
    if (rows[1].kind === "work-group") {
      expect(rows[1].key).toBe("wg-t1");
      expect(rows[1].active).toBe(false);
      expect(rows[1].msgs.map((m) => m.id)).toEqual(["t1", "t2"]);
      expect(workGroupSummary(rows[1].msgs)).toBe("Worked for 3s · 2 tool calls");
    }
    if (rows[3].kind === "tools") {
      expect(rows[3].msgs.map((m) => m.id)).toEqual(["t3"]);
    }
  });

  it("keeps a trailing live single tool as an active work group and flags hasLiveGroup", () => {
    const msgs = [
      user("u1"),
      tool("t1", "Read", "src/a.ts"),
      tool("t2", "Grep", "foo"),
      assistant("a1"),
      tool("t3", "Bash", "pnpm test", { state: "running", ended: null }),
    ];
    const { rows, hasLiveGroup } = buildDisplayRows(msgs, true, false, true);
    expect(hasLiveGroup).toBe(true);
    expect(rows.map((r) => r.kind)).toEqual(["msg", "work-group", "msg", "work-group"]);
    if (rows[3].kind === "work-group") {
      expect(rows[3].active).toBe(true);
      expect(rows[3].thinking).toBe(false);
    }
  });

  it("does not report hasLiveGroup when assistant text follows the tools while busy", () => {
    const msgs = [user("u1"), tool("t1"), tool("t2"), assistant("a1")];
    const { hasLiveGroup } = buildDisplayRows(msgs, true, false, true);
    expect(hasLiveGroup).toBe(false);
  });

  it("preserves flat tool runs when compactTools is disabled", () => {
    const msgs = [user("u1"), tool("t1"), tool("t2"), assistant("a1")];
    const { rows, hasLiveGroup } = buildDisplayRows(msgs, false, false, false);
    expect(hasLiveGroup).toBe(false);
    expect(rows.map((r) => r.kind)).toEqual(["msg", "tools", "msg"]);
  });
});

describe("describeStep & duration formatting", () => {
  it("matches native-gpui step descriptions", () => {
    expect(describeStep(null)).toBe("Working…");
    expect(describeStep(tool("t1", "Read", "src/ui/code_view.rs"), true)).toBe("Thinking…");
    expect(describeStep(tool("t1", "Read", "src/ui/code_view.rs"))).toBe("Reading code_view.rs");
    expect(describeStep(tool("t2", "Bash", "cargo test", { desc: "list project files" }))).toBe("List project files");
    expect(describeStep(tool("t3", "WebFetch", "https://docs.rs/gpui"))).toBe("Fetching docs.rs");
    expect(describeStep(tool("t4", "Write", ""))).toBe("Writing file");
    expect(describeStep(tool("t5", "winfer · bg", "start web-os static server"))).toBe("Start web-os static server");
  });

  it("formats elapsed durations and handles missing timestamps", () => {
    expect(elapsed(3000)).toBe("3s");
    expect(elapsed(65_000)).toBe("1m 05s");
    expect(elapsed(3_660_000)).toBe("1h 01m");
    expect(groupDurationMs([tool("t1", "Bash", "ls", { ts: null, ended: null })])).toBeNull();
    expect(workGroupSummary([tool("t1", "Bash", "ls", { ts: null, ended: null }), tool("t2", "Read", "a", { ts: null, ended: null })])).toBe("Worked · 2 tool calls");
  });
});
