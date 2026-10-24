/**
 * Clickable file paths in terminals: paths agents print (`Read(docs/PLAN.md)`, `src/a.ts:12`)
 * and OSC 8 hyperlinks to files (`file://…`, `vscode://file/…`). Click opens the file in the
 * side panel's viewer (web pages, .html and .svg, in winfer's browser); ⌘-click (Ctrl-click)
 * opens it in the code editor instead.
 */
import type { ILink, ILinkProvider, Terminal } from "@xterm/xterm";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { openInEditor } from "@/features/editors/editors";
import { openUrl as openInBrowser } from "@/features/browser/browser";
import { useWorkspaces } from "@/stores/workspaces";
import { fileFromUri, fileUrl, findPaths, isWebPage } from "./paths";
import { openFile, openFileFrom, resolveFiles } from "./viewer";

const IS_MAC = navigator.userAgent.includes("Mac");
const MOD = IS_MAC ? "⌘" : "Ctrl";
export const fileLinkHint = (path: string) =>
  `Click to open in the ${isWebPage(path) ? "browser" : "viewer"} · ${MOD}-click to open in the editor`;
/** How long a lookup is trusted: found files rarely vanish, missing ones often appear soon. */
const FOUND_MS = 60_000;
const MISSING_MS = 4_000;

const cache = new Map<string, { path: string | null; at: number }>();

/** Opens a found file: viewer (web pages in the browser), ⌘-click (Ctrl-click) in the editor. */
export function openFileLink(event: MouseEvent, path: string, line?: number) {
  if (IS_MAC ? event.metaKey : event.ctrlKey) void openInEditor(path);
  else if (isWebPage(path)) {
    useWorkspaces.getState().setSidePanel("browser");
    void openInBrowser(fileUrl(path), { newTab: true });
  } else openFile(path, { line });
}

/**
 * The whole logical line row `y` belongs to (rows a long line wrapped onto included), with the
 * buffer cell of each character (wide characters take two cells).
 */
function lineText(term: Terminal, y: number) {
  const buf = term.buffer.active;
  let top = y - 1;
  while (top > 0 && buf.getLine(top)?.isWrapped) top--;
  let text = "";
  const cells: { x: number; y: number }[] = [];
  for (let row = top; ; row++) {
    const line = buf.getLine(row);
    if (!line || (row > top && !line.isWrapped)) break;
    for (let x = 0; x < line.length; x++) {
      const cell = line.getCell(x);
      if (!cell) break;
      if (cell.getWidth() === 0) continue;
      const chars = cell.getChars() || " ";
      for (let i = 0; i < chars.length; i++)
        cells.push({ x: x + 1, y: row + 1 });
      text += chars;
    }
  }
  return { text, cells };
}

/** `paths` looked up from `cwd` (then `roots`), absolute when they name an existing file; cached briefly. */
export async function resolveCached(
  cwd: string,
  paths: string[],
  roots: string[] = [],
): Promise<(string | null)[]> {
  const now = Date.now();
  const key = (p: string) => `${cwd}\0${p}`;
  const fresh = (p: string) => {
    const hit = cache.get(key(p));
    return hit && now - hit.at < (hit.path ? FOUND_MS : MISSING_MS)
      ? hit
      : undefined;
  };
  const unknown = [...new Set(paths.filter((p) => !fresh(p)))];
  if (unknown.length) {
    const found = await resolveFiles(cwd, unknown, roots);
    unknown.forEach((p, i) => cache.set(key(p), { path: found[i], at: now }));
  }
  return paths.map((p) => cache.get(key(p))?.path ?? null);
}

/** Links for the paths on a terminal line that name existing files, looked up from the pane's folder. */
export function fileLinkProvider(
  term: Terminal,
  cwdOf: () => string,
  rootsOf: () => string[] = () => [],
): ILinkProvider {
  return {
    provideLinks(y, callback) {
      const line = lineText(term, y);
      // Only the paths that touch this row: the other rows ask for theirs.
      const matches = findPaths(line.text).filter(
        (m) => line.cells[m.start].y <= y && line.cells[m.end - 1].y >= y,
      );
      if (matches.length === 0) return callback(undefined);
      const cwd = cwdOf();
      const roots = rootsOf();
      void resolveCached(
        cwd,
        matches.map((m) => m.path),
        roots,
      ).then((found) => {
        const links: ILink[] = [];
        matches.forEach((m, i) => {
          const path = found[i];
          if (!path) return;
          links.push({
            range: { start: line.cells[m.start], end: line.cells[m.end - 1] },
            text: m.path,
            decorations: { pointerCursor: true, underline: true },
            activate: (event) => openFileLink(event, path, m.line),
            hover: (event) => {
              (event.target as HTMLElement | null)?.setAttribute(
                "title",
                fileLinkHint(path),
              );
            },
            leave: (event) => {
              (event.target as HTMLElement | null)?.removeAttribute("title");
            },
          });
        });
        callback(links.length ? links : undefined);
      });
    },
  };
}

/**
 * Opens a web link from a terminal in winfer's browser panel; ⌘-click (Ctrl-click) opens it
 * in the default browser instead. mailto: and other non-web links go to the system.
 */
export function openWebLink(event: MouseEvent | undefined, uri: string) {
  const external = event && (IS_MAC ? event.metaKey : event.ctrlKey);
  if (/^https?:/i.test(uri) && !external) {
    useWorkspaces.getState().setSidePanel("browser");
    void openInBrowser(uri, { newTab: true });
  } else void openExternal(uri);
}

/** Opens an OSC 8 hyperlink: files in the viewer (or editor), web links in winfer's browser. */
export function openTerminalUri(event: MouseEvent, uri: string, cwd: string) {
  const file = fileFromUri(uri);
  if (file) {
    if (IS_MAC ? event.metaKey : event.ctrlKey) {
      void resolveFiles(cwd, [file.path]).then(([p]) => {
        if (p) void openInEditor(p);
      });
    } else if (isWebPage(file.path)) {
      void resolveFiles(cwd, [file.path]).then(
        ([p]) => p && openFileLink(event, p),
      );
    } else void openFileFrom(cwd, file.path, { line: file.line });
    return;
  }
  if (/^(https?|mailto):/i.test(uri)) openWebLink(event, uri);
}
