/**
 * The side panel's file viewer: markdown rendered, images and PDFs shown, other text files with line
 * numbers. Opened by clicking a file path in a terminal, from a document's links, or by agents
 * through the `open_file` MCP tool.
 */
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { toast } from "@/stores/toasts";
import { useWorkspaces } from "@/stores/workspaces";
import { viewerProjects } from "./projects";

/** Asks the viewer to scroll to a line once the file shows; `nonce` repeats the same jump. */
export const useViewerJump = create<{ path: string | null; line: number | null; anchor: string | null; nonce: number }>(() => ({
  path: null,
  line: null,
  anchor: null,
  nonce: 0,
}));

/**
 * Absolute paths of the files `paths` name, null where there's no such file. Relative ones are
 * looked up from `cwd`, the folders above it in its checkout, then `roots`, then by name in the checkout.
 */
export function resolveFiles(cwd: string, paths: string[], roots?: string[]): Promise<(string | null)[]> {
  return invoke<(string | null)[]>("files_resolve", roots?.length ? { cwd, paths, roots } : { cwd, paths }).catch(() =>
    paths.map(() => null),
  );
}

/** Shows `path` (absolute) in the viewer tab of the side panel, opening the panel if needed. */
export function openFile(path: string, opts: { line?: number; anchor?: string; background?: boolean } = {}) {
  const s = useWorkspaces.getState();
  const docs = s.viewer.docs.includes(path) ? s.viewer.docs : [...s.viewer.docs, path];
  const groups = viewerProjects(docs, s.workspaces);
  const previous = groups.find((g) => s.viewer.active && g.docs.includes(s.viewer.active));
  const changedProject = previous && !previous.docs.includes(path);
  s.setViewer({
    ...s.viewer, docs, active: path,
    ...(changedProject && s.viewer.active && { activeByProject: { ...s.viewer.activeByProject, [previous.id]: s.viewer.active } }),
  });
  if (!opts.background) s.setSidePanel("viewer");
  useViewerJump.setState((j) => ({ path, line: opts.line ?? null, anchor: opts.anchor ?? null, nonce: j.nonce + 1 }));
}

/** Resolves `path` from `cwd` and opens it, or says it isn't there. */
export async function openFileFrom(cwd: string, path: string, opts: { line?: number; anchor?: string } = {}) {
  const [found] = await resolveFiles(cwd, [path]);
  if (found) openFile(found, opts);
  else toast(`Couldn't find ${path}`, "error");
  return found;
}

export function selectViewerProject(id: string) {
  const { viewer, workspaces, setViewer } = useWorkspaces.getState();
  const groups = viewerProjects(viewer.docs, workspaces);
  const target = groups.find((g) => g.id === id);
  if (!target) return;
  const current = groups.find((g) => viewer.active && g.docs.includes(viewer.active));
  const activeByProject = { ...viewer.activeByProject };
  if (current && viewer.active) activeByProject[current.id] = viewer.active;
  const remembered = activeByProject[id];
  setViewer({ ...viewer, activeByProject, active: target.docs.includes(remembered) ? remembered : target.docs[0] });
}

export function closeFile(path: string) {
  const { viewer, workspaces, setViewer } = useWorkspaces.getState();
  const docs = viewer.docs.filter((d) => d !== path);
  const group = viewerProjects(viewer.docs, workspaces).find((g) => g.docs.includes(path));
  const siblings = group?.docs.filter((d) => d !== path) ?? [];
  const candidates = siblings.length ? siblings : docs;
  const i = siblings.length ? group!.docs.indexOf(path) : viewer.docs.indexOf(path);
  const active = viewer.active === path ? (candidates[Math.min(i, candidates.length - 1)] ?? null) : viewer.active;
  const activeByProject = viewer.activeByProject && Object.fromEntries(Object.entries(viewer.activeByProject).filter(([, value]) => value !== path));
  setViewer({ ...viewer, docs, active, ...(activeByProject && { activeByProject }) });
}

/** Lets the user pick files, starting in the shown project (or active workspace). */
export async function pickFile(root?: string) {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId);
  const picked = await openDialog({
    multiple: true,
    directory: false,
    defaultPath: root ?? ws?.rootDir,
    filters: [
      { name: "Markdown", extensions: ["md", "markdown", "mdx", "txt"] },
      { name: "PDF", extensions: ["pdf"] },
      { name: "All files", extensions: ["*"] },
    ],
  }).catch(() => null);
  const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
  paths.forEach((p) => openFile(p));
}
