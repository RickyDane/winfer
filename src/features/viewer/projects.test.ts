import { beforeEach, describe, expect, it, vi } from "vitest";
import { useWorkspaces, type Workspace } from "@/stores/workspaces";
import { viewerProjects } from "./projects";
import { closeFile, openFile, selectViewerProject } from "./viewer";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const workspace = (id: string, rootDir: string): Workspace => ({
  id, name: id, rootDir, panes: [], layout: null, zoomedPaneId: null, presetIndex: 0, useWorktrees: true,
});

describe("file viewer projects", () => {
  beforeEach(() => {
    useWorkspaces.setState({
      hydrated: false,
      workspaces: [workspace("api", "/api"), workspace("web", "/web")],
      viewer: { docs: [], active: null },
    });
  });

  it("groups files by the most specific root, retaining external files", () => {
    const groups = viewerProjects(["/api/readme.md", "/api/nested/a.ts", "/api-copy/a.ts", "/web/a.ts"], [
      workspace("api", "/api"), workspace("nested", "/api/nested"), workspace("web", "/web"),
    ]);
    expect(groups.map((g) => [g.name, g.docs])).toEqual([
      ["api", ["/api/readme.md"]], ["nested", ["/api/nested/a.ts"]],
      ["Other files", ["/api-copy/a.ts"]], ["web", ["/web/a.ts"]],
    ]);
  });

  it("handles Windows separators, case, trailing slashes and drive roots", () => {
    expect(viewerProjects(["c:/REPO/docs/a.md", "C:\\repo\\b.ts"], [workspace("api", "C:\\Repo\\")])[0].docs).toHaveLength(2);
    expect(viewerProjects(["D:/a.md"], [workspace("drive", "D:/")])[0].id).toBe("drive");
    expect(viewerProjects(["/API/a.md"], [workspace("api", "/api")])[0].name).toBe("Other files");
  });

  it("groups pane worktrees under their parent project", () => {
    const api = workspace("api", "/api");
    api.panes = [{ worktree: { path: "/worktrees/task" } }] as Workspace["panes"];
    expect(viewerProjects(["/worktrees/task/a.md", "/api/b.md"], [api])[0].docs).toHaveLength(2);
  });

  it("remembers each project's selected file without switching the main workspace", () => {
    openFile("/api/a.md");
    openFile("/api/b.md");
    openFile("/web/a.md");
    selectViewerProject("api");
    expect(useWorkspaces.getState().viewer.active).toBe("/api/b.md");
    openFile("/api/b.md");
    selectViewerProject("web");
    const workspaceId = useWorkspaces.getState().activeWorkspaceId;
    selectViewerProject("api");
    expect(useWorkspaces.getState().viewer.active).toBe("/api/b.md");
    expect(useWorkspaces.getState().activeWorkspaceId).toBe(workspaceId);
  });

  it("keeps selection in the same project when closing a file, then falls back to another project", () => {
    openFile("/api/a.md");
    openFile("/web/a.md");
    openFile("/api/b.md");
    closeFile("/api/b.md");
    expect(useWorkspaces.getState().viewer.active).toBe("/api/a.md");
    closeFile("/api/a.md");
    expect(useWorkspaces.getState().viewer.active).toBe("/web/a.md");
    closeFile("/web/a.md");
    expect(useWorkspaces.getState().viewer).toMatchObject({ docs: [], active: null });
  });

  it("does not disturb selection when closing an inactive file or selecting a missing project", () => {
    openFile("/api/a.md");
    openFile("/web/a.md");
    closeFile("/api/a.md");
    selectViewerProject("missing");
    expect(useWorkspaces.getState().viewer.active).toBe("/web/a.md");
  });

  it("discards remembered files when closed and regroups files after project removal", () => {
    openFile("/api/a.md");
    openFile("/web/a.md");
    selectViewerProject("api");
    selectViewerProject("web");
    closeFile("/api/a.md");
    expect(useWorkspaces.getState().viewer.activeByProject?.api).toBeUndefined();
    useWorkspaces.setState({ workspaces: [] });
    expect(viewerProjects(useWorkspaces.getState().viewer.docs, [])[0].name).toBe("Other files");
  });
});
