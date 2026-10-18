import { invoke } from "@tauri-apps/api/core";
import { openUrl as openExternal, revealItemInDir } from "@tauri-apps/plugin-opener";
import { Code, Copy, ExternalLink, FileText, FolderOpen, Image as ImageIcon, Plus, RotateCw, X } from "lucide-react";
import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { Spinner } from "@/components/Spinner";
import { openUrl as openInBrowser } from "@/features/browser/browser";
import { openInEditor, preferredEditor, useEditors } from "@/features/editors/editors";
import { cn } from "@/lib/cn";
import { toast } from "@/stores/toasts";
import { useWorkspaces } from "@/stores/workspaces";
import { dirName, fileFromUri, fileName, isImage, isMarkdown, relativeTo, slugger } from "./paths";
import { closeFile, openFile, openFileFrom, pickFile, resolveFiles, useViewerJump } from "./viewer";

interface TextFile {
  path: string;
  text: string;
  modified: number;
}

/** How often an open file is checked for changes (agents edit the docs they show). */
const POLL_MS = 1500;
/** Code files longer than this are shown without line numbers. */
const MAX_NUMBERED_LINES = 20_000;

const IconButton = ({ title, onClick, active, children }: { title: string; onClick: () => void; active?: boolean; children: React.ReactNode }) => (
  <button title={title} onClick={onClick} className={cn("rounded p-1 hover:bg-hover hover:text-fg", active ? "bg-accent-soft text-accent" : "text-fg-subtle")}>
    {children}
  </button>
);

/** The file's name, with its folder when another open file has the same name. */
function tabLabel(path: string, docs: string[]) {
  const name = fileName(path);
  const twin = docs.some((d) => d !== path && fileName(d) === name);
  return twin ? `${fileName(dirName(path))}/${name}` : name;
}

const iconFor = (path: string) => (isMarkdown(path) ? FileText : isImage(path) ? ImageIcon : Code);

/** The side panel's file viewer: a tab per open file. */
export function ViewerPanel() {
  const { docs, active } = useWorkspaces((s) => s.viewer);
  const root = useWorkspaces((s) => s.workspaces.find((w) => w.id === s.activeWorkspaceId)?.rootDir);
  const shown = active && docs.includes(active) ? active : null;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border px-1.5">
        <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {docs.map((path) => {
            const Icon = iconFor(path);
            return (
              <button
                key={path}
                onClick={() => openFile(path)}
                onMouseDown={(e) => e.button === 1 && e.preventDefault()}
                onAuxClick={(e) => e.button === 1 && closeFile(path)}
                title={path}
                className={cn(
                  "group flex h-7 max-w-44 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs",
                  path === shown ? "bg-active text-fg" : "text-fg-muted hover:bg-hover",
                )}
              >
                <Icon size={12} className="shrink-0" />
                <span className="truncate">{tabLabel(path, docs)}</span>
                <X
                  size={11}
                  className="shrink-0 opacity-0 group-hover:opacity-60 hover:opacity-100"
                  onClick={(e) => {
                    e.stopPropagation();
                    closeFile(path);
                  }}
                />
              </button>
            );
          })}
        </div>
        <IconButton title="Open a file…" onClick={() => void pickFile()}>
          <Plus size={14} />
        </IconButton>
      </div>
      {shown ? (
        <FileView key={shown} path={shown} root={root} />
      ) : (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 text-center text-xs text-fg-subtle">
          <FileText size={22} className="text-fg-muted" />
          <span>Click a file path in a terminal to read it here: markdown is rendered, and the view follows the file as agents edit it.</span>
          <button onClick={() => void pickFile()} className="flex items-center gap-1.5 rounded-md bg-panel-raised px-3 py-1.5 text-xs font-medium text-fg hover:bg-hover">
            <FolderOpen size={13} /> Open a file…
          </button>
        </div>
      )}
    </div>
  );
}

function FileView({ path, root }: { path: string; root: string | undefined }) {
  const [file, setFile] = useState<TextFile | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [source, setSource] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const modified = useRef(0);
  const editor = preferredEditor(useEditors((s) => s.editors));
  const markdown = isMarkdown(path);
  const image = isImage(path);

  const load = useCallback(async () => {
    try {
      const next = await invoke<TextFile>("files_read_text", { path });
      modified.current = next.modified;
      setFile(next);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, [path]);

  useEffect(() => {
    if (image) return;
    void load();
    const timer = window.setInterval(() => {
      if (document.hidden) return;
      void invoke<number | null>("files_modified", { path }).then((m) => {
        if (m === null) setError(`${path} was deleted or moved`);
        else if (m !== modified.current) void load();
      });
    }, POLL_MS);
    return () => clearInterval(timer);
  }, [path, image, load]);

  // Scrolls to the line or heading asked for, once the content is there.
  const jump = useViewerJump();
  const done = useRef(-1);
  useLayoutEffect(() => {
    const box = scroller.current;
    if (!box || jump.path !== path || done.current === jump.nonce || (!file && !image)) return;
    done.current = jump.nonce;
    let target: Element | null = null;
    if (jump.anchor) target = box.querySelector(`[id="${CSS.escape(jump.anchor)}"]`);
    else if (jump.line) {
      if (markdown && !source) {
        for (const el of box.querySelectorAll<HTMLElement>("[data-line]")) {
          if (Number(el.dataset.line) > jump.line) break;
          target = el;
        }
      } else target = box.querySelector(`[data-row="${jump.line}"]`);
    }
    target?.scrollIntoView({ block: jump.anchor ? "start" : "center" });
  }, [jump, path, file, image, markdown, source]);

  const binary = file?.text.includes("\u0000");
  return (
    <>
      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-border px-2">
        <span title={path} className="mx-1 min-w-0 flex-1 truncate text-left font-mono text-[11px] text-fg-muted" dir="rtl">
          {/* rtl keeps the file name visible when the path is cut */}
          <bdi>{relativeTo(path, root)}</bdi>
        </span>
        {markdown && (
          <IconButton title={source ? "Show rendered" : "Show markdown source"} active={source} onClick={() => setSource(!source)}>
            <Code size={14} />
          </IconButton>
        )}
        {!image && (
          <IconButton title="Reload" onClick={() => void load()}>
            <RotateCw size={14} />
          </IconButton>
        )}
        <IconButton
          title="Copy path"
          onClick={() => void navigator.clipboard.writeText(path).then(() => toast("Path copied", "success"), () => toast("Couldn't copy the path", "error"))}
        >
          <Copy size={14} />
        </IconButton>
        <IconButton title="Show in folder" onClick={() => void revealItemInDir(path).catch((err) => toast("Couldn't show the file", "error", String(err)))}>
          <FolderOpen size={14} />
        </IconButton>
        <IconButton title={editor ? `Open in ${editor.name}` : "Open in editor"} onClick={() => void openInEditor(path)}>
          <ExternalLink size={14} />
        </IconButton>
      </div>
      <div ref={scroller} className="min-h-0 flex-1 overflow-auto select-text">
        {image ? (
          <div className="flex min-h-full items-center justify-center p-4">
            <LocalImage src={path} base={dirName(path)} alt={fileName(path)} />
          </div>
        ) : error && !file ? (
          <div className="p-6 text-center text-xs text-danger">{error}</div>
        ) : !file ? (
          <div className="flex h-full items-center justify-center text-xs text-fg-subtle">
            <Spinner size={14} />
          </div>
        ) : binary ? (
          <div className="p-6 text-center text-xs text-fg-subtle">This is a binary file. Open it in the editor to see it.</div>
        ) : markdown && !source ? (
          <>
            {error && <div className="bg-danger/10 px-3 py-1 text-[11px] text-danger">{error}</div>}
            <Markdown text={file.text} path={path} />
          </>
        ) : (
          <CodeView text={file.text} line={jump.path === path ? jump.line : null} />
        )}
      </div>
    </>
  );
}

function CodeView({ text, line }: { text: string; line: number | null }) {
  const lines = text.split(/\r?\n/);
  if (lines.length > MAX_NUMBERED_LINES) return <pre className="p-3 font-mono text-[12px] leading-5 whitespace-pre text-fg">{text}</pre>;
  return (
    <div className="py-2 font-mono text-[12px] leading-5">
      {lines.map((l, i) => (
        <div key={i} data-row={i + 1} className={cn("flex", line === i + 1 && "bg-accent-soft")}>
          <span className="w-12 shrink-0 pr-3 text-right text-fg-subtle select-none">{i + 1}</span>
          <span className="pr-4 whitespace-pre text-fg">{l || " "}</span>
        </div>
      ))}
    </div>
  );
}

interface HNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HNode[];
  position?: { start: { line: number } };
}

const textOf = (n: HNode): string => (n.type === "text" ? (n.value ?? "") : (n.children ?? []).map(textOf).join(""));

/** Tags elements with their source line (to scroll to `file.md:42`) and headings with GitHub-style ids. */
function rehypeLinesAndIds() {
  return (tree: HNode) => {
    const slug = slugger();
    const walk = (n: HNode) => {
      if (n.type === "element") {
        n.properties ??= {};
        if (n.position) n.properties.dataLine = n.position.start.line;
        if (/^h[1-6]$/.test(n.tagName ?? "")) n.properties.id = slug(textOf(n));
      }
      n.children?.forEach(walk);
    };
    walk(tree);
  };
}

const REMARK = [remarkGfm];
const REHYPE = [rehypeRaw, rehypeSanitize, rehypeLinesAndIds];

/** Follows a link in a document: anchors scroll, files open in the viewer, web pages in winfer's browser. */
function follow(href: string, docPath: string, box: HTMLElement | null) {
  if (href.startsWith("#")) {
    const id = decodeURIComponent(href.slice(1));
    box?.querySelector(`[id="${CSS.escape(id)}"]`)?.scrollIntoView({ block: "start" });
    return;
  }
  if (/^https?:/i.test(href)) {
    useWorkspaces.getState().setSidePanel("browser");
    void openInBrowser(href, { newTab: true });
    return;
  }
  if (/^mailto:/i.test(href)) {
    void openExternal(href);
    return;
  }
  // file:// and editor links; plain relative hrefs keep their #anchor.
  const file = /^[a-z][\w+.-]*:\/\//i.test(href) ? fileFromUri(href) : null;
  const [rawPath, anchor] = file ? [file.path, undefined] : href.split("#");
  let target = rawPath;
  try {
    target = decodeURIComponent(rawPath);
  } catch {}
  // A link to a line on GitHub-style docs: file.ts#L12.
  const line = file?.line ?? (anchor && /^L(\d+)/.exec(anchor)?.[1] ? Number(/^L(\d+)/.exec(anchor)![1]) : undefined);
  if (!target) return;
  void openFileFrom(dirName(docPath), target, { line, anchor: line ? undefined : anchor });
}

const Markdown = memo(function Markdown({ text, path }: { text: string; path: string }) {
  const box = useRef<HTMLDivElement>(null);
  const components: Components = {
    a: ({ href, children, node: _node, ...rest }) => (
      <a
        {...rest}
        href={href}
        title={href}
        onClick={(e) => {
          e.preventDefault();
          if (href) follow(href, path, box.current);
        }}
      >
        {children}
      </a>
    ),
    img: ({ src, alt, node: _node, ...rest }) => <LocalImage {...rest} src={typeof src === "string" ? src : undefined} alt={alt} base={dirName(path)} />,
  };
  return (
    <div ref={box} className="md-body px-6 py-5">
      <ReactMarkdown remarkPlugins={REMARK} rehypePlugins={REHYPE} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
});

const MIME: Record<string, string> = { svg: "image/svg+xml", png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", avif: "image/avif", bmp: "image/bmp", ico: "image/x-icon" };

/** An image from the web as is, or a local file (relative to `base`) read through the backend. */
function LocalImage({ src, base, ...rest }: React.ImgHTMLAttributes<HTMLImageElement> & { base: string }) {
  const remote = !!src && /^(https?:|data:|blob:)/i.test(src);
  const [url, setUrl] = useState<string | undefined>(remote ? src : undefined);
  useEffect(() => {
    if (!src || remote) return setUrl(src);
    let cancelled = false;
    let made: string | undefined;
    const file = fileFromUri(src)?.path ?? safeDecode(src.split(/[?#]/)[0]);
    void resolveFiles(base, [file]).then(async ([found]) => {
      if (!found || cancelled) return;
      const bytes = await invoke<ArrayBuffer>("files_read_bytes", { path: found }).catch(() => null);
      if (!bytes || cancelled) return;
      made = URL.createObjectURL(new Blob([bytes], { type: MIME[found.split(".").pop()!.toLowerCase()] ?? "" }));
      setUrl(made);
    });
    return () => {
      cancelled = true;
      if (made) URL.revokeObjectURL(made);
    };
  }, [src, base, remote]);
  return url ? <img {...rest} src={url} className="max-w-full" /> : <span className="text-fg-subtle">{rest.alt}</span>;
}

function safeDecode(s: string) {
  try {
    return decodeURI(s);
  } catch {
    return s;
  }
}
