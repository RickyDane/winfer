import { beforeEach, describe, expect, it, vi } from "vitest";

const storage = new Map<string, string>();
const setItem = vi.fn((key: string, value: string) => storage.set(key, value));
vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem });

beforeEach(() => {
  storage.clear();
  setItem.mockClear();
  vi.resetModules();
});

describe("private pane notes", () => {
  it("keeps panes independent, persists edits and removes cleared notes", async () => {
    const { usePaneNotes } = await import("./paneNotes");
    const { setNote } = usePaneNotes.getState();
    expect(setNote("one", "First task\nNext step")).toBe(true);
    setNote("two", "Second task");
    expect(JSON.parse(storage.get("winfer-pane-notes")!)).toEqual({ one: "First task\nNext step", two: "Second task" });
    vi.resetModules();
    const restored = (await import("./paneNotes")).usePaneNotes;
    expect(restored.getState().notes).toEqual(usePaneNotes.getState().notes);
    restored.getState().setNote("one", "");
    expect(restored.getState().notes).toEqual({ two: "Second task" });
    expect(JSON.parse(storage.get("winfer-pane-notes")!)).toEqual({ two: "Second task" });
  });

  it("ignores corrupt or invalid stored data", async () => {
    for (const value of ["broken", "null", "[]", "42"]) {
      storage.set("winfer-pane-notes", value);
      vi.resetModules();
      expect((await import("./paneNotes")).usePaneNotes.getState().notes).toEqual({});
    }
    storage.set("winfer-pane-notes", '{"one":"Keep","two":42}');
    vi.resetModules();
    expect((await import("./paneNotes")).usePaneNotes.getState().notes).toEqual({ one: "Keep" });
  });

  it("retains notes in memory and reports storage failures", async () => {
    const { usePaneNotes } = await import("./paneNotes");
    setItem.mockImplementationOnce(() => { throw new Error("Storage full"); });
    expect(usePaneNotes.getState().setNote("one", "Keep for now")).toBe(false);
    expect(usePaneNotes.getState().notes.one).toBe("Keep for now");
  });
});
