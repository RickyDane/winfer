import { beforeEach, describe, expect, it } from "vitest";
import { BINDINGS, chordFor, conflicts, rowChord, voiceChord } from "./keymap";
import { useWorkspaces } from "@/stores/workspaces";

const byTitle = (title: string) => BINDINGS.find((b) => b.title === title)!;

describe("keymap overrides", () => {
  beforeEach(() => useWorkspaces.setState({ shortcuts: {} }));

  it("gives every binding a stable id, shared only within numbered families", () => {
    const ids = new Map<string, Set<string>>();
    for (const b of BINDINGS) ids.set(b.id, (ids.get(b.id) ?? new Set()).add(b.digit ? b.title.replace(/\d$/, "") : b.title));
    for (const [id, titles] of ids) expect(titles.size, id).toBe(1);
    expect(byTitle("Focus pane 3").id).toBe("focus-pane");
  });

  it("uses an override, switches off with an empty chord, and expands families", () => {
    const newPane = byTitle("New pane (default agent)");
    useWorkspaces.getState().setShortcut(newPane.id, "Ctrl+Alt+N");
    expect(chordFor(newPane)).toBe("Ctrl+Alt+N");
    useWorkspaces.getState().setShortcut(newPane.id, "");
    expect(chordFor(newPane)).toBeNull();

    useWorkspaces.getState().setShortcut("focus-pane", "Alt+Shift+Meta+#");
    expect(chordFor(byTitle("Focus pane 7"))).toBe("Alt+Shift+Meta+7");
    expect(rowChord(byTitle("Focus pane 7"))).toBe("Alt+Shift+Meta+#");

    useWorkspaces.getState().setShortcut(newPane.id, null);
    expect(chordFor(newPane)).not.toBeNull();
  });

  it("finds conflicts, including family members and push-to-talk keys", () => {
    useWorkspaces.getState().setShortcut("focus-pane", "Alt+Shift+Meta+#");
    expect(conflicts("command-palette", "Alt+Shift+Meta+4").map((c) => c.id)).toEqual(["focus-pane"]);
    expect(conflicts("command-palette", "Alt+Space").map((c) => c.id)).toEqual(["voice-command"]);
    expect(conflicts("focus-pane", "Alt+Shift+Meta+#")).toEqual([]);
    expect(voiceChord("voice-dictation")).toBe("Alt+Shift+Space");
  });
});
