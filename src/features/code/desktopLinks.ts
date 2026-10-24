/** The desktop's LinkHost: paths resolve from the pane's folders and open in winfer's viewer/browser. */
import { fileLinkHint, openFileLink, openWebLink, resolveCached } from "@/features/viewer/terminalLinks";
import { useWorkspaces } from "@/stores/workspaces";
import type { LinkHost } from "./Links";

const MOD = navigator.userAgent.includes("Mac") ? "⌘" : "Ctrl";

function paneFolders(paneId: string) {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.panes.some((p) => p.id === paneId));
  const cwd = ws?.panes.find((p) => p.id === paneId)?.cwd ?? ws?.rootDir ?? "";
  return { cwd, roots: ws?.rootDir ? [ws.rootDir] : [] };
}

export function desktopLinks(paneId: string): LinkHost {
  return {
    resolve(paths) {
      const { cwd, roots } = paneFolders(paneId);
      return cwd ? resolveCached(cwd, paths, roots) : Promise.resolve(paths.map(() => null));
    },
    openWeb: openWebLink,
    openFile: openFileLink,
    fileHint: fileLinkHint,
    webHint: `Click to open in winfer's browser · ${MOD}-click to open in your browser`,
  };
}
