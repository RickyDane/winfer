import { describe, expect, it } from "vitest";
import { leaves } from "@/features/layout/tree";
import { archivedPanes, cleanAgentTitle, overviewOrder, useWorkspaces } from "./workspaces";

describe("agent titles", () => {
  it("drops spinner glyphs and titles that only name the CLI", () => {
    const names = ["Claude Code", "claude"];
    expect(cleanAgentTitle("✳ Fix the login redirect", names)).toBe("Fix the login redirect");
    expect(cleanAgentTitle("⠂  Refactor   auth guard", names)).toBe("Refactor auth guard");
    expect(cleanAgentTitle("✳ Claude Code", names)).toBeNull();
    expect(cleanAgentTitle("  ", names)).toBeNull();
    expect(cleanAgentTitle("Über-Seite bauen", names)).toBe("Über-Seite bauen");
  });

  it("drops console titles that are just a program's path", () => {
    expect(cleanAgentTitle("C:\\WINDOWS\\system32\\cmd.exe", [])).toBeNull();
    expect(cleanAgentTitle("Administrator: C:\\WINDOWS\\system32\\cmd.exe", [])).toBeNull();
    expect(cleanAgentTitle("C:\\Program Files\\PowerShell\\7\\pwsh.exe", [])).toBeNull();
    expect(cleanAgentTitle("/bin/zsh", [])).toBeNull();
    expect(cleanAgentTitle("Fix the /login redirect", [])).toBe("Fix the /login redirect");
  });
});

describe("archive", () => {
  const setup = () => {
    useWorkspaces.setState({ hydrated: false, view: "workspace", workspaces: [], activeWorkspaceId: "", focusedPaneId: null });
    const s = useWorkspaces.getState();
    s.createWorkspace("api", "/repo");
    const a = s.addPane("shell");
    const b = s.addPane("shell");
    return { a, b };
  };
  const ws = () => useWorkspaces.getState().workspaces[0];

  it("takes a pane out of the tiles but keeps it, and puts it back", () => {
    const { a, b } = setup();
    useWorkspaces.getState().archivePane(b);
    expect(leaves(ws().layout!)).toEqual([a]);
    expect(ws().panes.map((p) => p.id)).toEqual([a, b]);
    expect(archivedPanes(useWorkspaces.getState().workspaces).map((p) => p.id)).toEqual([b]);
    expect(overviewOrder(useWorkspaces.getState().workspaces)).toEqual([a]);
    // Focus moves to a pane still on screen.
    expect(useWorkspaces.getState().focusedPaneId).toBe(a);

    useWorkspaces.getState().unarchivePane(b);
    expect(leaves(ws().layout!).sort()).toEqual([a, b].sort());
    expect(ws().panes.find((p) => p.id === b)!.archived).toBe(false);
  });

  it("refocuses a tiled pane when leaving the archive", () => {
    const { a, b } = setup();
    useWorkspaces.getState().archivePane(b);
    useWorkspaces.setState({ view: "archive", focusedPaneId: b });
    useWorkspaces.getState().setView("workspace");
    expect(useWorkspaces.getState().focusedPaneId).toBe(a);
  });

  it("opens new panes in the tiles, leaving the archive", () => {
    setup();
    useWorkspaces.setState({ view: "archive" });
    useWorkspaces.getState().addPane("shell");
    expect(useWorkspaces.getState().view).toBe("workspace");
  });
});
