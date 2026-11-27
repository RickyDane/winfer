// The desktop's Files panel on the phone: the files open in its viewer and the workspace's folders;
// a file shows rendered (markdown), as numbered lines or as an image, and can be edited, commented
// on (`<!-- @user: … -->`, as on the desktop) and shown on the desktop.
import { useEffect, useMemo, useState } from "react";
import type { DesktopClient } from "../client";
import { haptic, useClient } from "../hooks";
import { forMachine } from "../multi";
import type { FileData, FileEntry, FilesOp } from "../protocol";
import { Icon, linkState } from "./common";
import { Markdown } from "./Markdown";
import { BarButton, Notice, PanelBar, call } from "./panelKit";
import { Sheet } from "./Pickers";
import { Segmented } from "./widgets";

type Listing = { root: string; dir: string; entries: FileEntry[] };
type Docs = { root: string; docs: string[]; active: string | null };

const base = (p: string) => p.replace(/\/+$/, "").split("/").pop() || p;
const parent = (p: string) => p.replace(/\/+$/, "").replace(/\/[^/]*$/, "") || "/";
/** `path` from the workspace's folder, or as is outside it. */
export const relative = (path: string, root: string) => (root && path.startsWith(root.replace(/\/+$/, "") + "/") ? path.slice(root.replace(/\/+$/, "").length + 1) : path);

/** Review comments shown as quotes in rendered markdown (they're HTML comments in the file). */
export function showComments(text: string): string {
  return text.replace(/<!--\s*@([\w-]+):\s*([\s\S]*?)\s*-->/g, (_, who: string, body: string) => `\n> 💬 **${who}:** ${body.replace(/\n/g, " ")}\n`);
}

const size = (n: number) => (n < 1024 ? `${n} B` : n < 1 << 20 ? `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB` : `${(n / (1 << 20)).toFixed(1)} MB`);

export function Files({ client, workspaceId }: { client: DesktopClient; workspaceId: string }) {
  const state = useClient(client);
  const ws = state.workspaces.find((w) => w.id === workspaceId);
  // The workspace's own machine, when several are paired.
  const online = linkState(forMachine(state, ws?.machine)) === "online";
  const [view, setView] = useState<"open" | "browse">("open");
  const [docs, setDocs] = useState<Docs | null>(null);
  const [listing, setListing] = useState<Listing | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const op = <T,>(o: FilesOp, extra: Record<string, unknown> = {}) => call<T>(client, { t: "files.call", op: o, workspaceId, ...extra });
  const loadDocs = () =>
    op<Docs>("docs")
      .then((d) => {
        setDocs(d);
        if (!d.docs.length && !listing) setView("browse");
      })
      .catch((e: Error) => setErr(e.message));
  const list = (dir?: string) =>
    op<Listing>("list", dir ? { dir } : {})
      .then((l) => {
        setListing(l);
        setErr(null);
      })
      .catch((e: Error) => setErr(e.message));

  useEffect(() => {
    if (!online) return;
    void loadDocs();
    void list(listing?.dir);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [online, workspaceId]);

  if (path) return <FileView client={client} workspaceId={workspaceId} path={path} root={docs?.root ?? listing?.root ?? ""} onBack={() => { setPath(null); void loadDocs(); }} />;

  const root = listing?.root ?? docs?.root ?? "";
  const atRoot = !listing || listing.dir.replace(/\/+$/, "") === listing.root.replace(/\/+$/, "");
  const back = () => listing && void list(parent(listing.dir));

  return (
    <div className="screen panel-screen">
      <PanelBar title="Files" subtitle={ws?.name} back={view === "browse" && !atRoot ? back : "/panel"}>
        <BarButton icon="refresh" label="Refresh" onClick={() => (view === "open" ? void loadDocs() : void list(listing?.dir))} />
      </PanelBar>
      <div className="filters">
        <Segmented
          options={[
            { value: "open", label: "Open", count: docs?.docs.length },
            { value: "browse", label: "Browse" },
          ]}
          value={view}
          onChange={setView}
        />
      </div>
      {err && <p className="inline-error panel-error">{err}</p>}
      <div className="scroll">
        {view === "open" ? (
          !docs ? (
            <Notice icon="file" title="Loading…" />
          ) : docs.docs.length === 0 ? (
            <Notice icon="file" title="No files open" text="Files agents show you, and ones you open here, appear in the desktop's Files panel." action={{ label: "Browse the workspace", onClick: () => setView("browse") }} />
          ) : (
            <div className="group-card file-list">
              {docs.docs.map((d) => (
                <div key={d} className="file-row">
                  <button className="row" onClick={() => setPath(d)}>
                    <span className="row-icon">
                      <Icon name="file" size={15} />
                    </span>
                    <span className="row-main">
                      <b>{base(d)}</b>
                      <span>
                        {relative(parent(d), root)}
                        {d === docs.active ? " · shown on the desktop" : ""}
                      </span>
                    </span>
                  </button>
                  <button className="icon-btn" aria-label="Close" title="Close on the desktop" onClick={() => void op("close", { path: d }).then(loadDocs)}>
                    <Icon name="x" size={15} />
                  </button>
                </div>
              ))}
            </div>
          )
        ) : !listing ? (
          <Notice icon="folder" title="Loading…" />
        ) : (
          <>
            <p className="file-crumb">{atRoot ? (ws?.name ?? base(listing.root)) : relative(listing.dir, listing.root)}</p>
            <div className="group-card file-list">
              {!atRoot && (
                <button className="row" onClick={() => void list(parent(listing.dir))}>
                  <span className="row-icon">
                    <Icon name="back" size={15} />
                  </span>
                  <span className="row-main">
                    <b>..</b>
                  </span>
                </button>
              )}
              {listing.entries.length === 0 && <p className="file-crumb">Empty folder</p>}
              {listing.entries.map((e) => {
                const full = `${listing.dir.replace(/\/+$/, "")}/${e.name}`;
                return (
                  <button key={e.name} className="row" onClick={() => (e.dir ? void list(full) : setPath(full))}>
                    <span className="row-icon">
                      <Icon name={e.dir ? "folder" : "file"} size={15} />
                    </span>
                    <span className="row-main">
                      <b>{e.name}</b>
                      {!e.dir && <span>{size(e.size)}</span>}
                    </span>
                    {e.dir && <Icon name="right" size={16} />}
                  </button>
                );
              })}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function FileView({ client, workspaceId, path, root, onBack }: { client: DesktopClient; workspaceId: string; path: string; root: string; onBack: () => void }) {
  const [file, setFile] = useState<FileData | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [source, setSource] = useState(false);
  const [draft, setDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [line, setLine] = useState<number | null>(null);
  const [comment, setComment] = useState("");
  const [note, setNote] = useState<string | null>(null);

  const op = <T,>(o: FilesOp, extra: Record<string, unknown> = {}) => call<T>(client, { t: "files.call", op: o, workspaceId, path, ...extra });
  const load = () =>
    op<FileData>("read")
      .then((f) => {
        setFile(f);
        setErr(null);
      })
      .catch((e: Error) => setErr(e.message));
  useEffect(() => {
    void load();
    const vis = () => document.visibilityState === "visible" && draft === null && void load();
    document.addEventListener("visibilitychange", vis);
    return () => document.removeEventListener("visibilitychange", vis);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  const text = file && "text" in file ? file.text : null;
  const lines = useMemo(() => (text ?? "").split("\n"), [text]);
  const markdown = file?.kind === "markdown";

  const save = async () => {
    if (draft === null || !file) return;
    setBusy(true);
    try {
      await op("write", { text: draft, expected: file.modified });
      haptic(8);
      setDraft(null);
      setNote("Saved");
      await load();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const addComment = async () => {
    if (line === null || !comment.trim() || !file) return;
    setBusy(true);
    try {
      await op("comment", { afterLine: line, body: comment, expected: file.modified });
      haptic(8);
      setLine(null);
      setComment("");
      setNote("Comment added");
      await load();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    if (!note) return;
    const t = setTimeout(() => setNote(null), 1800);
    return () => clearTimeout(t);
  }, [note]);

  const body = () => {
    if (!file) return err ? <Notice icon="alert" title="Can't open it" text={err} /> : <Notice icon="file" title="Loading…" />;
    if (file.kind === "image") return <div className="file-image"><img src={`data:${file.mime};base64,${file.data}`} alt={base(path)} /></div>;
    if (file.kind === "binary") return <Notice icon="file" title="Not a text file" text={`${size(file.size)} — open it on the desktop.`} />;
    if (file.kind === "large") return <Notice icon="file" title="Too big for the phone" text={`${size(file.size)} — open it on the desktop.`} />;
    if (draft !== null) return <textarea className="file-editor" value={draft} onChange={(e) => setDraft(e.target.value)} autoCapitalize="off" autoCorrect="off" spellCheck={false} />;
    if (markdown && !source)
      return (
        <div className="file-md">
          <Markdown text={showComments(file.text)} />
        </div>
      );
    return (
      <div className="file-code" role="list">
        {lines.map((l, i) => (
          <div key={i} className="file-line" role="listitem" data-on={line === i + 1 || undefined} onClick={() => setLine(i + 1)}>
            <span className="file-ln">{i + 1}</span>
            <span className="file-tx">{l || " "}</span>
          </div>
        ))}
      </div>
    );
  };

  const editable = file && (file.kind === "text" || file.kind === "markdown");
  return (
    <div className="screen panel-screen">
      <PanelBar title={base(path)} subtitle={relative(parent(path), root)} back={draft !== null ? () => setDraft(null) : onBack}>
        {draft !== null ? (
          <BarButton icon="save" label="Save" disabled={busy} onClick={() => void save()} />
        ) : (
          <>
            {markdown && <BarButton icon={source ? "eye" : "code"} label={source ? "Rendered" : "Source (tap a line to comment)"} onClick={() => setSource((s) => !s)} />}
            {editable && <BarButton icon="pencil" label="Edit" onClick={() => text !== null && setDraft(text)} />}
            <BarButton
              icon="open"
              label="Show on the desktop"
              onClick={() =>
                void op("open")
                  .then(() => setNote("Shown in the desktop's Files panel"))
                  .catch((e: Error) => setErr(e.message))
              }
            />
          </>
        )}
      </PanelBar>
      {err && file && (
        <p className="inline-error panel-error">
          {err}{" "}
          <button className="btn sm ghost" onClick={() => { setErr(null); setDraft(null); void load(); }}>
            Reload
          </button>
        </p>
      )}
      {note && <p className="panel-toast">{note}</p>}
      <div className="scroll file-scroll">{body()}</div>
      {line !== null && draft === null && editable && (
        <Sheet title={`Comment after line ${line}`} sub={lines[line - 1]?.trim().slice(0, 80) || undefined} label="Comment" onClose={() => setLine(null)}>
          <div className="sheet-body file-comment">
            <textarea value={comment} onChange={(e) => setComment(e.target.value)} placeholder="Your comment for the agent" autoFocus />
            <button className="btn primary block" disabled={busy || !comment.trim()} onClick={() => void addComment()}>
              {busy ? "Adding…" : "Add comment"}
            </button>
          </div>
        </Sheet>
      )}
    </div>
  );
}
