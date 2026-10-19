import { invoke } from "@tauri-apps/api/core";
import { openUrl as openExternal, revealItemInDir } from "@tauri-apps/plugin-opener";
import { Check, Code, Copy, ExternalLink, FileText, FolderOpen, Image as ImageIcon, MessageSquarePlus, Pencil, Plus, RotateCw, Send, X } from "lucide-react";
import { Fragment, memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ThemedToken } from "shiki";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { Spinner } from "@/components/Spinner";
import { openUrl as openInBrowser } from "@/features/browser/browser";
import { openInEditor, preferredEditor, useEditors } from "@/features/editors/editors";
import { cn } from "@/lib/cn";
import { toast } from "@/stores/toasts";
import { leaves } from "@/features/layout/tree";
import { pasteText, sendText } from "@/features/terminal/xtermRegistry";
import { providerName, useWorkspaces } from "@/stores/workspaces";
import { findComments, insertComment, parseCommentValue, removeComment } from "./comments";
import { languageFor, lineTokens, tokenStyle, useHighlight } from "./highlight";
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
  /** The text being edited, and when the file last changed as editing started. */
  const [draft, setDraft] = useState<{ text: string; base: number } | null>(null);
  const [conflict, setConflict] = useState(false);
  /** The block hovered (to comment on) and the one being commented on. */
  const [hover, setHover] = useState<{ line: number; top: number; left: number } | null>(null);
  const [composing, setComposing] = useState<{ line: number; quote: string } | null>(null);
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
      return next;
    } catch (err) {
      setError(String(err));
      return null;
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

  /** Writes `text`, refusing when the file changed since `expected` (unless null). */
  const write = useCallback(
    async (text: string, expected: number | null) => {
      const m = await invoke<number>("files_write_text", { path, text, expected });
      modified.current = m;
      setFile({ path, text, modified: m });
      setError(null);
    },
    [path],
  );

  /** Changes the file as it is on disk now, so an agent's latest edits aren't lost. */
  const update = useCallback(
    async (change: (text: string) => string, shown: string) => {
      const fresh = await load();
      if (!fresh) return toast("Couldn't read the file", "error");
      if (fresh.text !== shown) return toast("The file just changed, try again", "error");
      try {
        await write(change(fresh.text), fresh.modified);
      } catch (err) {
        toast("Couldn't save the file", "error", String(err) === "changed" ? "It changed while saving. Try again." : String(err));
        void load();
      }
    },
    [load, write],
  );

  const saveDraft = useCallback(
    async (force = false) => {
      if (!draft) return;
      try {
        await write(draft.text, force ? null : draft.base);
        setDraft(null);
        setConflict(false);
        toast("Saved", "success");
      } catch (err) {
        if (String(err) === "changed") setConflict(true);
        else toast("Couldn't save the file", "error", String(err));
      }
    },
    [draft, write],
  );

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

  // Marks the block being commented on.
  useEffect(() => {
    if (!composing) return;
    const el = scroller.current?.querySelector(`.md-body > [data-end="${composing.line}"]`);
    el?.classList.add("md-commenting");
    return () => el?.classList.remove("md-commenting");
  }, [composing, file]);

  const binary = file?.text.includes("\u0000");
  const editable = !image && !!file && !binary;
  const rendered = markdown && !source && !draft;
  const shownText = file?.text ?? "";
  const userComments = markdown ? findComments(shownText).filter((c) => c.author === "user").length : 0;

  const onRemoveComment = useCallback((at: number) => void update((t) => removeComment(t, at), shownText), [update, shownText]);

  const trackHover = (e: React.MouseEvent) => {
    const wrap = e.currentTarget as HTMLElement;
    const el = (e.target as Element).closest<HTMLElement>(".md-body > [data-end]:not(.md-comment)");
    if (!el) return;
    const line = Number(el.dataset.end);
    if (hover?.line === line) return;
    const w = wrap.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    setHover({ line, top: r.top - w.top, left: Math.max(2, r.left - w.left - 26) });
  };

  const startComment = (line: number) => {
    const el = scroller.current?.querySelector<HTMLElement>(`.md-body > [data-end="${line}"]`);
    const quote = (el?.textContent ?? "").replace(/\s+/g, " ").trim();
    setComposing({ line, quote: quote.length > 90 ? `${quote.slice(0, 90)}…` : quote });
  };

  return (
    <>
      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-border px-2">
        <span title={path} className="mx-1 min-w-0 flex-1 truncate text-left font-mono text-[11px] text-fg-muted" dir="rtl">
          {/* rtl keeps the file name visible when the path is cut */}
          <bdi>{relativeTo(path, root)}</bdi>
        </span>
        {draft ? (
          <>
            <button onClick={() => void saveDraft()} className="flex items-center gap-1 rounded-md bg-accent px-2 py-1 text-xs font-medium text-white hover:opacity-90" title="Save (⌘S)">
              <Check size={13} /> Save
            </button>
            <button onClick={() => (setDraft(null), setConflict(false))} className="rounded-md px-2 py-1 text-xs text-fg-muted hover:bg-hover" title="Discard your changes (Esc)">
              Cancel
            </button>
          </>
        ) : (
          <>
            {markdown && userComments > 0 && <SendComments path={path} count={userComments} />}
            {markdown && (
              <IconButton title={source ? "Show rendered" : "Show markdown source"} active={source} onClick={() => setSource(!source)}>
                <Code size={14} />
              </IconButton>
            )}
            {editable && (
              <IconButton title="Edit" onClick={() => (setComposing(null), setDraft({ text: file.text, base: file.modified }))}>
                <Pencil size={14} />
              </IconButton>
            )}
            {!image && (
              <IconButton title="Reload" onClick={() => void load()}>
                <RotateCw size={14} />
              </IconButton>
            )}
          </>
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
      {draft && file && file.modified !== draft.base && (
        <div className="flex shrink-0 items-center gap-2 bg-warning/10 px-3 py-1.5 text-[11px] text-fg">
          <span className="flex-1">{conflict ? "The file changed on disk since you started editing." : "The file changed on disk while you were editing."}</span>
          <button className="rounded px-1.5 py-0.5 hover:bg-hover" onClick={() => (setDraft({ text: file.text, base: file.modified }), setConflict(false))}>
            Load theirs
          </button>
          {conflict && (
            <button className="rounded px-1.5 py-0.5 font-medium text-danger hover:bg-hover" onClick={() => void saveDraft(true)}>
              Overwrite
            </button>
          )}
        </div>
      )}
      <div ref={scroller} className="relative min-h-0 flex-1 overflow-auto select-text">
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
        ) : draft ? (
          <textarea
            autoFocus
            spellCheck={markdown}
            value={draft.text}
            onChange={(e) => setDraft({ ...draft, text: e.target.value })}
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && e.key === "s") {
                e.preventDefault();
                void saveDraft();
              } else if (e.key === "Escape" && draft.text === file.text) setDraft(null);
            }}
            style={{ fontVariantLigatures: "none" }}
            className="block h-full w-full resize-none bg-transparent p-4 font-mono text-[12px] leading-5 text-fg outline-none"
          />
        ) : rendered ? (
          <div className="relative" onMouseMove={trackHover} onMouseLeave={() => setHover(null)}>
            {error && <div className="bg-danger/10 px-3 py-1 text-[11px] text-danger">{error}</div>}
            <Markdown text={file.text} path={path} onRemoveComment={onRemoveComment} />
            {hover && !composing && (
              <button
                title="Comment on this part (the agent sees it in the file)"
                onClick={() => startComment(hover.line)}
                style={{ top: hover.top, left: hover.left }}
                className="absolute rounded p-1 text-fg-subtle hover:bg-hover hover:text-accent"
              >
                <MessageSquarePlus size={14} />
              </button>
            )}
          </div>
        ) : (
          <CodeView text={file.text} line={jump.path === path ? jump.line : null} lang={languageFor(path)} />
        )}
      </div>
      {composing && rendered && (
        <CommentComposer
          quote={composing.quote}
          onCancel={() => setComposing(null)}
          onSubmit={(body) => {
            const line = composing.line;
            setComposing(null);
            void update((t) => insertComment(t, line, body), shownText);
          }}
        />
      )}
    </>
  );
}

/** Where the user writes a comment on the block they picked. */
function CommentComposer({ quote, onSubmit, onCancel }: { quote: string; onSubmit: (body: string) => void; onCancel: () => void }) {
  const [body, setBody] = useState("");
  const submit = () => body.trim() && onSubmit(body);
  return (
    <div className="shrink-0 border-t border-border bg-panel-raised p-2">
      {quote && <div className="mb-1.5 truncate border-l-2 border-accent pl-2 text-[11px] text-fg-muted">{quote}</div>}
      <textarea
        autoFocus
        rows={3}
        value={body}
        placeholder="Comment for the agent…"
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            submit();
          } else if (e.key === "Escape") onCancel();
        }}
        className="block w-full resize-none rounded-md border border-border bg-panel px-2 py-1.5 text-xs text-fg outline-none focus:border-accent"
      />
      <div className="mt-1.5 flex items-center justify-end gap-1.5">
        <span className="mr-auto text-[10px] text-fg-subtle">Saved in the file as an HTML comment · ⌘↵</span>
        <button onClick={onCancel} className="rounded-md px-2 py-1 text-xs text-fg-muted hover:bg-hover">
          Cancel
        </button>
        <button onClick={submit} disabled={!body.trim()} className="rounded-md bg-accent px-2 py-1 text-xs font-medium text-white hover:opacity-90 disabled:opacity-40">
          Comment
        </button>
      </div>
    </div>
  );
}

/** Asks an agent in the workspace to go through the user's comments in the file. */
function SendComments({ path, count }: { path: string; count: number }) {
  const [open, setOpen] = useState(false);
  const ws = useWorkspaces((s) => s.workspaces.find((w) => w.id === s.activeWorkspaceId));
  const providers = useWorkspaces((s) => s.providers);
  const panes = ws?.layout ? leaves(ws.layout).flatMap((id) => ws.panes.filter((p) => p.id === id)) : [];
  const send = (paneId: string) => {
    setOpen(false);
    const text = `I left ${count === 1 ? "a review comment" : `${count} review comments`} in ${path} (HTML comments like <!-- @user: … --> after the part they're about). Read the file, address them, and remove each comment once it's handled.`;
    pasteText(paneId, text);
    // Lets the paste land before submitting it.
    window.setTimeout(() => sendText(paneId, "\r"), 80);
    toast("Comments sent to the agent", "success");
  };
  return (
    <div className="relative">
      <IconButton title={`Send ${count} comment${count === 1 ? "" : "s"} to an agent`} active={open} onClick={() => setOpen(!open)}>
        <span className="flex items-center gap-0.5">
          <Send size={13} />
          <span className="text-[10px] font-medium">{count}</span>
        </span>
      </IconButton>
      {open && (
        <div className="absolute top-full right-0 z-20 mt-1 w-56 rounded-md border border-border bg-panel-raised p-1 shadow-lg" onMouseLeave={() => setOpen(false)}>
          <div className="px-2 py-1 text-[10px] text-fg-subtle">Ask an agent to address your comments</div>
          {panes.length === 0 && <div className="px-2 py-1 text-xs text-fg-muted">No panes in this workspace</div>}
          {panes.map((p, i) => (
            <button key={p.id} onClick={() => send(p.id)} className="flex w-full items-center gap-2 rounded px-2 py-1 text-left text-xs text-fg hover:bg-hover">
              <span className="text-fg-subtle">{i + 1}</span>
              <span className="truncate">{providerName(providers, p.provider)}</span>
              <span className="ml-auto text-[10px] text-fg-subtle">{p.status}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** A review comment kept in the document, shown as a note. */
function CommentNote({ author, at, children, onRemove }: { author: string; at: number; children: React.ReactNode; onRemove: (at: number) => void }) {
  const user = author === "user";
  return (
    <span className={cn("md-comment group", user ? "md-comment-user" : "md-comment-agent")} contentEditable={false}>
      <span className="md-comment-head">
        <span className="font-medium">{user ? "You" : author}</span>
        <button title={user ? "Delete comment" : "Resolve (remove from the file)"} onClick={() => onRemove(at)} className="ml-auto rounded p-0.5 opacity-0 group-hover:opacity-100 hover:bg-hover">
          {user ? <X size={12} /> : <Check size={12} />}
        </button>
      </span>
      <span className="whitespace-pre-wrap">{children}</span>
    </span>
  );
}

function CodeView({ text, line, lang }: { text: string; line: number | null; lang: string | null }) {
  const lines = text.split(/\r?\n/);
  const tokens = useHighlight(text, lines.length > MAX_NUMBERED_LINES ? null : lang);
  if (lines.length > MAX_NUMBERED_LINES) return <pre className="p-3 font-mono text-[12px] leading-5 whitespace-pre text-fg">{text}</pre>;
  return (
    <div className="py-2 font-mono text-[12px] leading-5">
      {lines.map((l, i) => (
        <div key={i} data-row={i + 1} className={cn("flex", line === i + 1 && "bg-accent-soft")}>
          <span className="w-12 shrink-0 pr-3 text-right text-fg-subtle select-none">{i + 1}</span>
          <span className="pr-4 whitespace-pre text-fg">{l ? <Tokenized line={l} tokens={lineTokens(tokens, i, l)} /> : " "}</span>
        </div>
      ))}
    </div>
  );
}

function Tokenized({ line, tokens }: { line: string; tokens: ThemedToken[] | null }) {
  if (!tokens) return line;
  return tokens.map((t, j) => (
    <span key={j} className="shiki-tok" style={tokenStyle(t)}>
      {t.content}
    </span>
  ));
}

/** A fenced code block in markdown, highlighted when its language is known. */
function FencedCode({ className, children, ...rest }: React.ComponentProps<"code">) {
  const lang = /language-([\w+#.-]+)/.exec(className ?? "")?.[1] ?? null;
  const code = typeof children === "string" ? children.replace(/\n$/, "") : null;
  const tokens = useHighlight(code ?? "", code === null ? null : lang);
  if (!tokens || code === null) return <code className={className} {...rest}>{children}</code>;
  const lines = code.split(/\r?\n/);
  return (
    <code className={className} {...rest}>
      {lines.map((l, i) => (
        <Fragment key={i}>
          {i > 0 && "\n"}
          <Tokenized line={l} tokens={lineTokens(tokens, i, l)} />
        </Fragment>
      ))}
    </code>
  );
}

interface HNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HNode[];
  position?: { start: { line: number; offset?: number }; end: { line: number } };
}

const textOf = (n: HNode): string => (n.type === "text" ? (n.value ?? "") : (n.children ?? []).map(textOf).join(""));

/**
 * Tags elements with their source line (to scroll to `file.md:42`), top-level blocks with their
 * last line (to comment after them), headings with GitHub-style ids, and turns review comments
 * (`<!-- @user: … -->`) into notes that know where they are in the source.
 */
function rehypeLinesAndIds() {
  return (tree: HNode) => {
    const slug = slugger();
    const walk = (n: HNode, top: boolean) => {
      if (n.type === "element") {
        n.properties ??= {};
        if (n.position) n.properties.dataLine = n.position.start.line;
        if (top && n.position) n.properties.dataEnd = n.position.end.line;
        if (/^h[1-6]$/.test(n.tagName ?? "")) n.properties.id = slug(textOf(n));
      }
      n.children = n.children?.map((c) => {
        const comment = c.type === "comment" && c.position?.start.offset !== undefined ? parseCommentValue(c.value ?? "") : null;
        if (!comment) return c;
        return {
          type: "element",
          tagName: "span",
          properties: { dataCommentAt: c.position!.start.offset, dataAuthor: comment.author },
          children: [{ type: "text", value: comment.body }],
        };
      });
      n.children?.forEach((c) => walk(c, n.type === "root"));
    };
    walk(tree, false);
  };
}

const REMARK = [remarkGfm];
/** Sanitized as usual, but keeping HTML comments: review comments are shown. */
const SANITIZE = { ...defaultSchema, allowComments: true };
const REHYPE: Options["rehypePlugins"] = [rehypeRaw, [rehypeSanitize, SANITIZE], rehypeLinesAndIds];

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

const Markdown = memo(function Markdown({ text, path, onRemoveComment }: { text: string; path: string; onRemoveComment: (at: number) => void }) {
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
    span: ({ node, children, ...rest }) => {
      const at = node?.properties?.dataCommentAt;
      if (typeof at !== "number") return <span {...rest}>{children}</span>;
      return (
        <CommentNote author={String(node?.properties?.dataAuthor ?? "")} at={at} onRemove={onRemoveComment}>
          {children}
        </CommentNote>
      );
    },
    code: ({ node: _node, ...rest }) => <FencedCode {...rest} />,
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
