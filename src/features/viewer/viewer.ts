/**
 * The side panel's file viewer: markdown rendered, images shown, other text files with line
 * numbers. Opened by clicking a file path in a terminal, from a document's links, or by agents
 * through the `open_file` MCP tool.
 */
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { toast } from "@/stores/toasts";
import { useWorkspaces } from "@/stores/workspaces";

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
  s.setViewer({ docs, active: path });
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

export function closeFile(path: string) {
  const { viewer, setViewer } = useWorkspaces.getState();
  const docs = viewer.docs.filter((d) => d !== path);
  const i = viewer.docs.indexOf(path);
  const active = viewer.active === path ? (docs[Math.min(i, docs.length - 1)] ?? null) : viewer.active;
  setViewer({ docs, active });
}

/** Lets the user pick files to view, starting in the active workspace's folder. */
export async function pickFile() {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId);
  const picked = await openDialog({
    multiple: true,
    directory: false,
    defaultPath: ws?.rootDir,
    filters: [
      { name: "Markdown", extensions: ["md", "markdown", "mdx", "txt"] },
      { name: "All files", extensions: ["*"] },
    ],
  }).catch(() => null);
  const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
  paths.forEach((p) => openFile(p));
}
