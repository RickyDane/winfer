/**
 * Clickable file paths in terminals: paths agents print (`Read(docs/PLAN.md)`, `src/a.ts:12`)
 * and OSC 8 hyperlinks to files (`file://…`, `vscode://file/…`). Click opens the file in the
 * side panel's viewer; ⌘-click (Ctrl-click) opens it in the code editor instead.
 */
import type { ILink, ILinkProvider, Terminal } from "@xterm/xterm";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { openInEditor } from "@/features/editors/editors";
import { fileFromUri, findPaths } from "./paths";
import { openFile, openFileFrom, resolveFiles } from "./viewer";

const IS_MAC = navigator.userAgent.includes("Mac");
const HINT = `Click to open in the viewer · ${IS_MAC ? "⌘" : "Ctrl"}-click to open in the editor`;
/** How long a lookup is trusted: found files rarely vanish, missing ones often appear soon. */
const FOUND_MS = 60_000;
const MISSING_MS = 4_000;

const cache = new Map<string, { path: string | null; at: number }>();

function open(event: MouseEvent, path: string, line?: number) {
  if (IS_MAC ? event.metaKey : event.ctrlKey) void openInEditor(path);
  else openFile(path, { line });
}

/** A buffer line's text, with the cell column of each character (wide characters take two cells). */
function lineText(term: Terminal, y: number) {
  const line = term.buffer.active.getLine(y - 1);
  if (!line) return null;
  let text = "";
  const cols: number[] = [];
  for (let x = 0; x < line.length; x++) {
    const cell = line.getCell(x);
    if (!cell) break;
    if (cell.getWidth() === 0) continue;
    const chars = cell.getChars() || " ";
    for (let i = 0; i < chars.length; i++) cols.push(x);
    text += chars;
  }
  return { text, cols };
}

/** Links for the paths on a terminal line that name existing files, looked up from the pane's folder. */
export function fileLinkProvider(term: Terminal, cwdOf: () => string): ILinkProvider {
  return {
    provideLinks(y, callback) {
      const line = lineText(term, y);
      const matches = line ? findPaths(line.text) : [];
      if (!line || matches.length === 0) return callback(undefined);
      const cwd = cwdOf();
      const now = Date.now();
      const key = (p: string) => `${cwd}\0${p}`;
      const fresh = (p: string) => {
        const hit = cache.get(key(p));
        return hit && now - hit.at < (hit.path ? FOUND_MS : MISSING_MS) ? hit : undefined;
      };
      const unknown = [...new Set(matches.map((m) => m.path).filter((p) => !fresh(p)))];
      void (unknown.length ? resolveFiles(cwd, unknown) : Promise.resolve([])).then((found) => {
        unknown.forEach((p, i) => cache.set(key(p), { path: found[i], at: now }));
        const links: ILink[] = [];
        for (const m of matches) {
          const path = cache.get(key(m.path))?.path;
          if (!path) continue;
          links.push({
            range: { start: { x: line.cols[m.start] + 1, y }, end: { x: line.cols[m.end - 1] + 1, y } },
            text: m.path,
            decorations: { pointerCursor: true, underline: true },
            activate: (event) => open(event, path, m.line),
            hover: (event) => {
              (event.target as HTMLElement | null)?.setAttribute("title", HINT);
            },
            leave: (event) => {
              (event.target as HTMLElement | null)?.removeAttribute("title");
            },
          });
        }
        callback(links.length ? links : undefined);
      });
    },
  };
}

/** Opens an OSC 8 hyperlink: files in the viewer (or editor), web links in the default browser. */
export function openTerminalUri(event: MouseEvent, uri: string, cwd: string) {
  const file = fileFromUri(uri);
  if (file) {
    if (IS_MAC ? event.metaKey : event.ctrlKey) {
      void resolveFiles(cwd, [file.path]).then(([p]) => {
        if (p) void openInEditor(p);
      });
    } else void openFileFrom(cwd, file.path, { line: file.line });
    return;
  }
  if (/^(https?|mailto):/i.test(uri)) void openExternal(uri);
}
