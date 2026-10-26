import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  ArrowDown,
  Bot,
  Brain,
  Check,
  ChevronRight,
  Copy,
  FilePen,
  FileText,
  FolderSearch,
  Globe,
  Info,
  Search,
  SquareTerminal,
  TriangleAlert,
  Wrench,
  X,
} from "lucide-react";
import clsx from "clsx";
import { useCode, type Item } from "./store";
import { Markdown } from "./Markdown";
import { Linkify } from "./Links";
import { Welcome } from "./Welcome";
import { HISTORY_PAGE, historyStart } from "./history";
import { followAfterScroll } from "./scrollFollow";
import { useCodeWeb } from "./styles";
import { modelName } from "./names";
import { elapsed } from "../usage/toolStats";

const TOOL_ICONS: Record<string, typeof Bot> = {
  Read: FileText,
  Grep: Search,
  Glob: FolderSearch,
  Bash: SquareTerminal,
  Edit: FilePen,
  Write: FilePen,
  Agent: Bot,
  WebFetch: Globe,
  WebSearch: Globe,
};

function useTicker(on: boolean) {
  const [, set] = useState(0);
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => set((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, [on]);
}

const secs = elapsed;

/**
 * Animated open/close via a grid-rows transition, so content height animates without measuring.
 * The content mounts the first time it opens (a long session's collapsed tool output would
 * otherwise all sit in the DOM) and stays mounted, so closing still animates.
 */
export function Collapse({
  open,
  children,
}: {
  open: boolean;
  children: ReactNode;
}) {
  const [mounted, setMounted] = useState(open);
  if (open && !mounted) setMounted(true);
  return (
    <div className={clsx("cw-collapse", open && "open")}>
      <div>{mounted && children}</div>
    </div>
  );
}

type DiffRow = {
  t: "add" | "del" | "ctx" | "hunk";
  o?: number;
  n?: number;
  text: string;
};

function diffRows(diff: string): DiffRow[] {
  let oldN = 0;
  let newN = 0;
  const rows: DiffRow[] = [];
  for (const line of diff.split("\n")) {
    if (line.startsWith("---") || line.startsWith("+++")) continue;
    const h = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);
    if (h) {
      oldN = +h[1];
      newN = +h[2];
      rows.push({ t: "hunk", text: line });
    } else if (line.startsWith("+"))
      rows.push({ t: "add", n: newN++, text: line.slice(1) });
    else if (line.startsWith("-"))
      rows.push({ t: "del", o: oldN++, text: line.slice(1) });
    else rows.push({ t: "ctx", o: oldN++, n: newN++, text: line.slice(1) });
  }
  return rows;
}

export const DiffView = memo(function DiffView({ diff, max }: { diff: string; max?: number }) {
  const rows = useMemo(() => diffRows(diff), [diff]);
  const shown = max ? rows.slice(0, max) : rows;
  return (
    <div className="cw-diff">
      {shown.map((r, i) => (
        <div
          key={i}
          className={clsx("cw-diff-row", r.t)}
          style={{ animationDelay: `${Math.min(i, 30) * 12}ms` }}
        >
          <span className="ln">{r.o ?? ""}</span>
          <span className="ln">{r.n ?? ""}</span>
          <span className="sign">
            {r.t === "add" ? "+" : r.t === "del" ? "−" : ""}
          </span>
          <span className="txt">{r.text || " "}</span>
        </div>
      ))}
      {max && rows.length > max && (
        <div className="cw-diff-more">… {rows.length - max} more lines</div>
      )}
    </div>
  );
});

function diffStat(diff: string) {
  let a = 0;
  let d = 0;
  for (const l of diff.split("\n")) {
    if (l.startsWith("+") && !l.startsWith("+++")) a++;
    else if (l.startsWith("-") && !l.startsWith("---")) d++;
  }
  return { a, d };
}

/** True while the user has text marked, so click-to-toggle handlers don't fire at the end of a drag. */
const selecting = () => !!window.getSelection()?.toString();

/** A quiet, discoverable footer action that never covers message content. */
function CopyMessage({ text }: { text: string }) {
  const [status, setStatus] = useState<"idle" | "copying" | "copied" | "error">("idle");
  useEffect(() => {
    if (status !== "copied" && status !== "error") return;
    const timer = setTimeout(() => setStatus("idle"), 2400);
    return () => clearTimeout(timer);
  }, [status]);
  const label = status === "copied" ? "Copied" : status === "error" ? "Retry copy" : "Copy";
  return (
    <div className="cw-msg-actions">
      <button
        type="button"
        className={clsx("cw-msg-copy", status === "copied" && "copied")}
        title={status === "error" ? "Couldn't copy. Try again." : "Copy message"}
        aria-label="Copy message"
        disabled={status === "copying"}
        onClick={async () => {
          setStatus("copying");
          try {
            await navigator.clipboard.writeText(text);
            setStatus("copied");
          } catch {
            setStatus("error");
          }
        }}
      >
        {status === "copied" ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
        <span role="status">{label}</span>
      </button>
    </div>
  );
}

/** Notices (e.g. "stopped background task …" with its whole command) stay at two lines; click to expand. */
function NoticeText({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <span
      className={clsx("cw-notice-text", open && "open")}
      onClick={() => !selecting() && setOpen(!open)}
    >
      <Linkify text={text} />
    </span>
  );
}

// Rows are memoized on their item: items are immutable and a streamed token replaces only the
// item it belongs to, so the rest of a long transcript doesn't re-render on every token.
const ToolCard = memo(function ToolCard({ item }: { item: Extract<Item, { kind: "tool" }> }) {
  const running = item.ok === undefined;
  const isShell = item.name === "Bash";
  const [open, setOpen] = useState<boolean | null>(null);
  const expandShell = useCodeWeb((s) => s.expandShell);
  const expanded = open ?? (isShell && running && expandShell);
  useTicker(running);
  const Icon = TOOL_ICONS[item.name] ?? Wrench;
  const task = useCode((s) =>
    Object.values(s.tasks).find((t) => t.tool_id === item.key),
  );
  const openLog = useCode((s) => s.openTaskLog);
  const outRef = useRef<HTMLPreElement>(null);
  // Also on opening: the output only mounts then.
  useEffect(() => {
    if (outRef.current) outRef.current.scrollTop = outRef.current.scrollHeight;
  }, [item.output, expanded]);
  const stat = useMemo(() => (item.diff ? diffStat(item.diff) : null), [item.diff]);
  const command = (item.input as { command?: string } | null)?.command;
  const hasBody = !!item.output || !!item.diff || !!command;

  return (
    <div
      className={clsx(
        "cw-tool cw-enter",
        running && "running",
        item.ok === false && "failed",
      )}
    >
      <button
        className="cw-tool-head"
        onClick={() => hasBody && setOpen(!expanded)}
      >
        <span className="cw-tool-icon">
          <Icon size={14} />
        </span>
        <span className="cw-tool-name">{item.name}</span>
        <span className="cw-tool-title">
          <Linkify text={item.title} />
        </span>
        {task && (
          <span
            className="cw-chip clickable"
            title="Show its output"
            onClick={(e) => {
              e.stopPropagation();
              openLog(task.id);
            }}
          >
            {task.agent
              ? `${task.agent}${task.model ? ` · ${modelName(task.model, {})}` : ""} · ${task.tools ?? 0} tools`
              : "background"}
            {task.status === "running"
              ? task.last
                ? ` · ${task.last}`
                : " · running"
              : ` · ${task.status}`}
          </span>
        )}
        {stat && (
          <span className="cw-stat">
            <b className="add">+{stat.a}</b> <b className="del">−{stat.d}</b>
          </span>
        )}
        {(item.ended ?? Date.now()) - item.started > 0 && (
          <span className="cw-tool-meta">
            {secs((item.ended ?? Date.now()) - item.started)}
          </span>
        )}
        <span className="cw-tool-status">
          {running ? (
            <span className="cw-spinner" />
          ) : item.ok ? (
            <Check size={14} />
          ) : (
            <X size={14} />
          )}
        </span>
        {hasBody && (
          <ChevronRight
            size={14}
            className={clsx("cw-chev", expanded && "open")}
          />
        )}
      </button>
      <Collapse open={expanded && hasBody}>
        {item.diff ? (
          <DiffView diff={item.diff} />
        ) : (
          <pre ref={outRef} className={clsx("cw-tool-out", isShell && "shell")}>
            {command && <span className="cw-prompt">$ {command}</span>}
            <Linkify text={item.output} />
          </pre>
        )}
      </Collapse>
    </div>
  );
});

const Reasoning = memo(function Reasoning({ item }: { item: Extract<Item, { kind: "reasoning" }> }) {
  const [open, setOpen] = useState(useCodeWeb.getState().expandThinking);
  useTicker(item.streaming);
  const took = secs((item.ended ?? Date.now()) - item.started);
  return (
    <div className="cw-reason cw-enter">
      <button className="cw-reason-head" onClick={() => setOpen(!open)}>
        <Brain size={14} />
        <span className={clsx(item.streaming && "cw-shimmer")}>
          {item.streaming ? "Thinking" : `Thought for ${took}`}
        </span>
        <ChevronRight size={13} className={clsx("cw-chev", open && "open")} />
      </button>
      <Collapse open={open}>
        <div className="cw-reason-body">
          <Linkify text={item.text} />
        </div>
      </Collapse>
    </div>
  );
});

/** "Worked 12s" / "Worked 3m 05s" / "Worked 1h 02m". */
function fmtWorked(ms: number) {
  const t = Math.max(1, Math.round(ms / 1000));
  const h = Math.floor(t / 3600),
    m = Math.floor((t % 3600) / 60),
    sec = t % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `Worked ${h ? `${h}h ${pad(m)}m` : m ? `${m}m ${pad(sec)}s` : `${sec}s`}`;
}

const UserRow = memo(function UserRow({ item }: { item: Extract<Item, { kind: "user" }> }) {
  const setPreview = useCode((s) => s.setPreview);
  return (
    <div className="cw-user cw-enter">
      <div className="cw-user-bubble">
        {item.images && (
          <div className="cw-user-images">
            {item.images.map((u, i) => (
              <img
                key={i}
                src={u}
                alt=""
                onClick={() => setPreview({ images: item.images!, index: i })}
                title="Preview"
              />
            ))}
          </div>
        )}
        <Linkify text={item.text} />
      </div>
      {item.text && <CopyMessage text={item.text} />}
    </div>
  );
});

const TextRow = memo(function TextRow({ item, dark }: { item: Extract<Item, { kind: "text" }>; dark: boolean }) {
  return (
    <div className={clsx("cw-assistant cw-enter", item.streaming && "streaming")}>
      <Markdown text={item.text} dark={dark} streaming={item.streaming} />
      {!item.streaming && item.text && <CopyMessage text={item.text} />}
    </div>
  );
});

const NoticeRow = memo(function NoticeRow({ item }: { item: Extract<Item, { kind: "notice" }> }) {
  return (
    <div className={clsx("cw-notice cw-enter", item.tone)}>
      {item.tone === "error" ? <TriangleAlert size={13} /> : <Info size={13} />}
      <NoticeText text={item.text} />
    </div>
  );
});

/** No turn has run yet: the welcome screen is showing. */
export function useFresh() {
  return useCode(
    (s) =>
      !s.busy &&
      !s.items.some(
        (i) => i.kind === "user" || i.kind === "text" || i.kind === "tool",
      ),
  );
}

export const Transcript = memo(function Transcript({ dark }: { dark: boolean }) {
  const items = useCode((s) => s.items);
  const busy = useCode((s) => s.busy);
  const fresh = useFresh();
  const scroller = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const lastTop = useRef(0);
  const scrollIntent = useRef(0);
  const touchY = useRef<number | undefined>(undefined);
  const lastUser = useRef<string | undefined>(undefined);
  const [away, setAway] = useState(false);
  const head = items[0]?.key;
  const [history, setHistory] = useState(() => ({ head, start: historyStart(items.length) }));
  const start = historyStart(items.length, history.head === head ? history.start : undefined);
  if (history.head !== head || history.start !== start) setHistory({ head, start });
  const prepend = useRef<{ row: Element; top: number } | null>(null);
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && prepend.current) {
      const { row: anchor, top } = prepend.current;
      // Mounted rows have real heights, so the previous first row can be anchored
      // directly, without estimates that change later as the page is revealed.
      el.scrollTo({ top: el.scrollTop + anchor.getBoundingClientRect().top - top, behavior: "instant" });
      prepend.current = null;
    }
  }, [start]);

  // ResizeObserver and token commits often arrive together. Read scrollHeight and
  // write scrollTop once per frame, never from inside the observer's layout phase.
  const followFrame = useRef(0);
  const follow = () => {
    followFrame.current ||= requestAnimationFrame(() => {
      followFrame.current = 0;
      if (stick.current) toBottom();
    });
  };

  // Streaming follows instantly; only an explicit Jump to latest animates.
  const toBottom = (behavior: ScrollBehavior = "instant") => {
    const el = scroller.current;
    if (el) el.scrollTo({ top: el.scrollHeight, behavior });
  };

  const scrolling = (direction: number) => {
    scrollIntent.current = Math.sign(direction);
    const el = scroller.current;
    if (direction <= 0) {
      const wasFollowing = stick.current;
      stick.current = false;
      cancelAnimationFrame(followFrame.current);
      followFrame.current = 0;
      // Cancel an in-flight Jump to latest animation before native wheel/touch
      // scrolling starts. Otherwise it can keep dragging the reader downward.
      if (wasFollowing && el) el.scrollTo({ top: el.scrollTop, behavior: "instant" });
      setAway(true);
    } else if (el && el.scrollHeight - el.scrollTop - el.clientHeight <= 2) {
      stick.current = true;
      setAway(false);
    }
  };

  useEffect(() => {
    // A newly sent message always snaps back to the bottom, even if scrolled up.
    let user: string | undefined;
    for (let i = items.length - 1; i >= 0 && !user; i--)
      if (items[i].kind === "user") user = items[i].key;
    if (user !== lastUser.current) {
      stick.current = true;
      scrollIntent.current = 0;
      lastUser.current = user;
      setAway(false);
    }
    if (stick.current) {
      // Long-running panes need the same bound as restored ones. Only retire old
      // rows while following, never while the user is reading earlier history.
      const latest = historyStart(items.length, start, true);
      if (latest !== start) setHistory({ head, start: latest });
      follow();
    }
  }, [items]);

  // Keep following when content grows after render (markdown, images, tool output).
  useEffect(() => {
    const box = inner.current;
    if (!box) return;
    const ro = new ResizeObserver(() => {
      if (stick.current) follow();
    });
    ro.observe(box);
    // Height-only pane resizing doesn't necessarily resize the content itself.
    if (scroller.current) ro.observe(scroller.current);
    return () => {
      ro.disconnect();
      cancelAnimationFrame(followFrame.current);
      followFrame.current = 0;
    };
  }, []);

  return (
    <div
      className="cw-scroll"
      ref={scroller}
      tabIndex={0}
      onWheel={(e) => {
        if (!e.ctrlKey && !e.metaKey && e.deltaY) scrolling(e.deltaY);
      }}
      onPointerDown={(e) => {
        // Scrollbar dragging does not emit wheel events. Pause before it starts;
        // subsequent downward movement to the bottom may resume following.
        if (e.target === e.currentTarget) scrolling(0);
      }}
      onTouchStart={(e) => {
        touchY.current = e.touches[0]?.clientY;
        scrolling(0);
      }}
      onTouchMove={(e) => {
        const y = e.touches[0]?.clientY;
        if (y !== undefined && touchY.current !== undefined && y !== touchY.current)
          scrolling(touchY.current - y);
        touchY.current = y;
      }}
      onKeyDown={(e) => {
        if ((e.target as HTMLElement).closest("input, textarea, [contenteditable=true]")) return;
        if (["ArrowUp", "PageUp", "Home"].includes(e.key) || (e.key === " " && e.shiftKey)) scrolling(-1);
        else if (["ArrowDown", "PageDown", "End", " "].includes(e.key)) scrolling(1);
      }}
      onScroll={(e) => {
        const el = e.currentTarget;
        const dist = el.scrollHeight - el.scrollTop - el.clientHeight;
        stick.current = followAfterScroll(stick.current, lastTop.current, el.scrollTop, dist, scrollIntent.current);
        lastTop.current = el.scrollTop;
        setAway(!stick.current);
      }}
    >
      {fresh && <Welcome />}
      <div className="cw-transcript" ref={inner}>
        {start > 0 && (
          <button
            className="cw-history-more"
            onClick={() => {
              const el = scroller.current;
              if (!el) return;
              stick.current = false;
              scrollIntent.current = -1;
              setAway(true);
              const row = inner.current?.children[1];
              if (row) prepend.current = { row, top: row.getBoundingClientRect().top };
              setHistory({ head, start: Math.max(0, start - HISTORY_PAGE) });
            }}
          >
            Show earlier messages ({start})
          </button>
        )}
        {items.slice(start).map((it) => {
          switch (it.kind) {
            case "user":
              return <UserRow key={it.key} item={it} />;
            case "text":
              return <TextRow key={it.key} item={it} dark={dark} />;
            case "reasoning":
              return <Reasoning key={it.key} item={it} />;
            case "tool":
              return <ToolCard key={it.key} item={it} />;
            case "worked":
              return (
                <div key={it.key} className="cw-worked cw-enter">
                  <span>{fmtWorked(it.ms)}</span>
                </div>
              );
            case "notice":
              return <NoticeRow key={it.key} item={it} />;
          }
        })}
        {busy && items.at(-1)?.kind === "user" && (
          <div className="cw-reason cw-enter">
            <div className="cw-reason-head">
              <span className="cw-dots">
                <i />
                <i />
                <i />
              </span>
            </div>
          </div>
        )}
        <div style={{ height: 1 }} />
      </div>
      {away && (
        <div className="cw-jump-wrap">
          <button
            className="cw-jump"
            aria-label="Jump to latest"
            title="Jump to latest"
            onClick={() => {
              stick.current = true;
              scrollIntent.current = 0;
              setAway(false);
              setHistory({ head, start: historyStart(items.length) });
              toBottom("smooth");
            }}
          >
            <ArrowDown size={16} />
          </button>
        </div>
      )}
    </div>
  );
});
