/** Code editors a project or worktree can be opened in (see src-tauri/src/editors.rs). */
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { toast } from "@/stores/toasts";
import { useWorkspaces } from "@/stores/workspaces";

export interface Editor {
  id: string;
  name: string;
}

/** Editors installed on this machine, found once at startup. */
export const useEditors = create<{ editors: Editor[] }>(() => ({ editors: [] }));

export async function loadEditors() {
  const editors = await invoke<Editor[]>("editors_list").catch(() => []);
  useEditors.setState({ editors });
}

/** The editor chosen in Settings if it's installed, else the first one found. */
export function preferredEditor(editors = useEditors.getState().editors, chosen = useWorkspaces.getState().editor): Editor | undefined {
  return editors.find((e) => e.id === chosen) ?? editors[0];
}

/** Opens `path` in `editorId`, or the preferred editor. */
export async function openInEditor(path: string, editorId?: string) {
  const editor = editorId ? useEditors.getState().editors.find((e) => e.id === editorId) : preferredEditor();
  if (!editor) {
    toast("No code editor found", "error", "Install VS Code, Cursor, Zed or another editor, then restart winfer.");
    return;
  }
  try {
    await invoke("editor_open", { id: editor.id, path });
  } catch (err) {
    toast(`Couldn't open ${editor.name}`, "error", String(err));
  }
}
