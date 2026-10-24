/**
 * Clickable URLs and file paths in the web UI, like the TUI's terminal links: a path opens in the
 * viewer (web pages in winfer's browser), a URL in winfer's browser; ⌘-click (Ctrl-click) opens
 * the file in the editor or the URL in the default browser. Paths link only once they resolve
 * to an existing file, from the pane's folder or its workspace folder.
 */
import {
  Children,
  createContext,
  useContext,
  useEffect,
  useState,
  type MouseEvent,
  type ReactNode,
} from "react";
import { fileFromUri, findPaths } from "@/features/viewer/paths";

/** http(s) URLs, minus trailing sentence punctuation and an unbalanced closing bracket. */
const URL_RE = /\bhttps?:\/\/[^\s<>"'`]+/gi;

function trimUrl(url: string) {
  let u = url.replace(/[.,;:!?]+$/, "");
  for (const [open, close] of [
    ["(", ")"],
    ["[", "]"],
  ]) {
    while (u.endsWith(close) && u.split(open).length < u.split(close).length)
      u = u.slice(0, -1);
  }
  return u;
}

/** Where links go: winfer's browser and viewer on the desktop (desktopLinks.ts), tabs on the phone. */
export interface LinkHost {
  /** Absolute paths of the files `paths` name (relative to the session's folder), null if none. */
  resolve(paths: string[]): Promise<(string | null)[]>;
  openWeb(e: globalThis.MouseEvent, url: string): void;
  openFile(e: globalThis.MouseEvent, abs: string, line?: number): void;
  fileHint(abs: string): string;
  webHint: string;
}

export const LinkHostContext = createContext<LinkHost | null>(null);

type Piece = {
  text: string;
  url?: string;
  path?: string;
  line?: number;
  start: number;
};

function split(text: string): Piece[] {
  const marks: Piece[] = [];
  for (const m of text.matchAll(URL_RE))
    marks.push({ text: trimUrl(m[0]), url: trimUrl(m[0]), start: m.index });
  for (const p of findPaths(text)) {
    if (
      marks.some(
        (u) => u.url && p.start < u.start + u.text.length && p.end > u.start,
      )
    )
      continue;
    marks.push({
      text: text.slice(p.start, p.end),
      path: p.path,
      line: p.line,
      start: p.start,
    });
  }
  marks.sort((a, b) => a.start - b.start);
  const out: Piece[] = [];
  let at = 0;
  for (const m of marks) {
    if (m.start < at) continue;
    if (m.start > at) out.push({ text: text.slice(at, m.start), start: at });
    out.push(m);
    at = m.start + m.text.length;
  }
  if (at < text.length) out.push({ text: text.slice(at), start: at });
  return out;
}

const stop = (e: MouseEvent) => {
  e.preventDefault();
  e.stopPropagation();
};

/** `text` with its URLs and existing file paths as links. */
export function Linkify({ text }: { text: string }) {
  const host = useContext(LinkHostContext);
  const pieces = split(text);
  const paths = pieces.filter((p) => p.path).map((p) => p.path!);
  const key = paths.join("\0");
  const [found, setFound] = useState<Record<string, string | null>>({});

  useEffect(() => {
    if (!paths.length || !host) return;
    let live = true;
    void host.resolve(paths).then((abs) => {
      if (live) setFound(Object.fromEntries(paths.map((p, i) => [p, abs[i]])));
    });
    return () => {
      live = false;
    };
  }, [key, host]);

  if (!pieces.some((p) => p.url || p.path)) return <>{text}</>;
  return (
    <>
      {pieces.map((p) => {
        if (p.url) {
          return (
            <a
              key={p.start}
              href={p.url}
              className="cw-link"
              title={host?.webHint}
              target="_blank"
              rel="noreferrer"
              onClick={(e) => {
                if (!host) return;
                stop(e);
                host.openWeb(e.nativeEvent, p.url!);
              }}
            >
              {p.text}
            </a>
          );
        }
        const abs = p.path ? found[p.path] : null;
        if (!abs || !host) return <span key={p.start}>{p.text}</span>;
        return (
          <a
            key={p.start}
            href="#"
            className="cw-link"
            title={host.fileHint(abs)}
            onClick={(e) => {
              stop(e);
              host.openFile(e.nativeEvent, abs, p.line);
            }}
          >
            {p.text}
          </a>
        );
      })}
    </>
  );
}

/** Children with their plain-text parts linkified (elements are left as they are). */
export function linkifyChildren(children: ReactNode): ReactNode {
  return Children.map(children, (c) =>
    typeof c === "string" ? <Linkify text={c} /> : c,
  );
}

/** A markdown link: web URLs in winfer's browser, file links in the viewer. */
export function MarkdownLink({
  href,
  children,
}: {
  href?: string;
  children?: ReactNode;
}) {
  const host = useContext(LinkHostContext);
  return (
    <a
      href={href}
      className="cw-link"
      title={href}
      target="_blank"
      rel="noreferrer"
      onClick={(e) => {
        if (!host) return;
        stop(e);
        if (!href) return;
        if (/^(https?|mailto):/i.test(href))
          return host.openWeb(e.nativeEvent, href);
        const file =
          fileFromUri(href) ??
          (/^[a-z][\w+.-]*:/i.test(href)
            ? null
            : { path: decodeURI(href.split("#")[0]), line: undefined });
        if (!file?.path) return;
        const native = e.nativeEvent;
        void host
          .resolve([file.path])
          .then(([abs]) => abs && host.openFile(native, abs, file.line));
      }}
    >
      {children}
    </a>
  );
}
