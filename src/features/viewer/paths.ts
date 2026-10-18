/** Finding file paths in terminal text and in links, for the file viewer. Pure: no Tauri. */

export interface PathMatch {
  /** The path as written (without a `:line:col` suffix). */
  path: string;
  line?: number;
  /** Where the match (suffix included) starts and ends in the text, end exclusive. */
  start: number;
  end: number;
}

/** A file mention: optional drive, `~`, `./` or `../`, folders, then a name with an extension that has a letter in it
 * (so `1.2.3` doesn't count), then an optional `:line`, `:line:col`, `:from-to` (OpenCode) or `#L12` (Codex). */
const PATH = /(?<![\w/\\.~@%+-])((?:[A-Za-z]:[\\/]|~[\\/]|\.{1,2}[\\/]|[\\/])?(?:[\w@+-][\w.@+-]*[\\/])*[\w@+-][\w.@+-]*\.(?=[A-Za-z0-9]*[A-Za-z])[A-Za-z0-9]{1,12})(?:(?::|#L)(\d+)(?:[:-]L?\d+)?)?(?![\w\\/@+-])/g;
const WEB_URL = /\b[a-z][\w+.-]*:\/\/\S+/gi;

/** File-path-looking mentions in a line of text, leaving out anything inside a URL. */
export function findPaths(text: string): PathMatch[] {
  const urls = [...text.matchAll(WEB_URL)].map((m) => [m.index, m.index + m[0].length]);
  const out: PathMatch[] = [];
  for (const m of text.matchAll(PATH)) {
    const start = m.index;
    const end = start + m[0].length;
    if (urls.some(([a, b]) => start < b && end > a)) continue;
    // "foo.md." at the end of a sentence: the regex can't end on a dot, so this is already clean.
    out.push({ path: m[1], line: m[2] ? Number(m[2]) : undefined, start, end });
  }
  return out;
}

const MARKDOWN = /\.(md|markdown|mdown|mkd|mkdn|mdx)$/i;
const IMAGE = /\.(png|jpe?g|gif|webp|svg|bmp|ico|avif)$/i;

export const isMarkdown = (path: string) => MARKDOWN.test(path);
export const isImage = (path: string) => IMAGE.test(path);

export const fileName = (path: string) => path.split(/[\\/]/).pop() || path;
export const dirName = (path: string) => path.replace(/[\\/][^\\/]*$/, "") || path;

/** `path` relative to `root` when it's inside it. */
export function relativeTo(path: string, root: string | undefined) {
  if (!root) return path;
  const base = root.replace(/[\\/]+$/, "");
  return path.startsWith(base) && /[\\/]/.test(path[base.length] ?? "") ? path.slice(base.length + 1) : path;
}

/**
 * The local file a link points to: `file:///abs/path`, or the editor links agents print
 * (`vscode://file/abs/path:12:3`, also cursor, windsurf, zed…). Null for web and other links.
 */
export function fileFromUri(uri: string): { path: string; line?: number } | null {
  let rest: string;
  if (/^file:\/\//i.test(uri)) {
    try {
      const url = new URL(uri);
      if (url.host && url.host !== "localhost") return null;
      rest = decodeURIComponent(url.pathname);
      const line = /^L?(\d+)/.exec(url.hash.slice(1))?.[1];
      // file:///C:/x on Windows
      if (/^\/[A-Za-z]:\//.test(rest)) rest = rest.slice(1);
      return { path: rest, line: line ? Number(line) : undefined };
    } catch {
      return null;
    }
  }
  // A bare href (OpenCode and Claude Code keep markdown links' targets as written): a path from the pane's folder.
  if (!/^[a-z][\w+.-]*:/i.test(uri) || /^[A-Za-z]:[\\/]/.test(uri)) {
    const bare = /^(.*?)(?:(?::|#L)(\d+)(?:[:-]L?\d+)?)?$/.exec(uri.split(/[?]/)[0])!;
    let path = bare[1].replace(/#.*$/, "");
    try {
      path = decodeURIComponent(path);
    } catch {}
    return path ? { path, line: bare[2] ? Number(bare[2]) : undefined } : null;
  }
  const editor = /^(?:vscode(?:-insiders)?|cursor|windsurf|vscodium|zed|kiro):\/\/file(\/.*)$/i.exec(uri);
  if (!editor) return null;
  try {
    rest = decodeURIComponent(editor[1]);
  } catch {
    rest = editor[1];
  }
  if (/^\/[A-Za-z]:[\\/]/.test(rest)) rest = rest.slice(1);
  const pos = /^(.*?)(?::(\d+))?(?::\d+)?$/.exec(rest)!;
  return { path: pos[1], line: pos[2] ? Number(pos[2]) : undefined };
}

/** GitHub-style heading anchors: "Getting started!" → "getting-started", repeats get -1, -2…. */
export function slugger() {
  const seen = new Map<string, number>();
  return (text: string) => {
    const base = text
      .toLowerCase()
      .trim()
      .replace(/[^\p{L}\p{N}\s_-]/gu, "")
      .replace(/\s/g, "-");
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    return n ? `${base}-${n}` : base;
  };
}
