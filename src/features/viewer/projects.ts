import type { Workspace } from "@/stores/workspaces";

export interface ViewerProject {
  id: string;
  name: string;
  root?: string;
  docs: string[];
}

const normalized = (path: string) => {
  const value = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return /^[a-z]:/i.test(value) || value.startsWith("//") ? value.toLowerCase() : value;
};

/** Most specific project root wins; a pane's worktree belongs to its parent project. */
export function viewerProjects(docs: string[], workspaces: Workspace[]): ViewerProject[] {
  const roots = workspaces.flatMap((w) => [w.rootDir, ...w.panes.flatMap((p) => p.worktree ? [p.worktree.path] : [])]
    .map((root) => ({ workspace: w, root: normalized(root) })))
    .sort((a, b) => b.root.length - a.root.length);
  const groups = new Map<string, ViewerProject>();
  for (const path of docs) {
    const file = normalized(path);
    const owner = roots.find(({ root }) => file.startsWith(`${root}/`))?.workspace;
    const id = owner?.id ?? "__other_files__";
    let group = groups.get(id);
    if (!group) {
      group = { id, name: owner?.name ?? "Other files", root: owner?.rootDir, docs: [] };
      groups.set(id, group);
    }
    group.docs.push(path);
  }
  return [...groups.values()];
}
