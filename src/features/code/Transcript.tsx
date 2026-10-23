import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Bot,
  Brain,
  Check,
  ChevronRight,
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
import { useCodeWeb } from "./styles";

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
    const t = setInterval(() => set((n) => n + 1), 100);
    return () => clearInterval(t);
  }, [on]);
}

const secs = (ms: number) => (ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`);

/** Animated open/close via a grid-rows transition, so content height animates without measuring. */
export function Collapse({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div className={clsx("cw-collapse", open && "open")}>
      <div>{children}</div>
    </div>
  );
}

export function DiffView({ diff, max }: { diff: string; max?: number }) {
  let oldN = 0;
  let newN = 0;
  const rows: { t: "add" | "del" | "ctx" | "hunk"; o?: number; n?: number; text: string }[] = [];
  for (const line of diff.split("\n")) {
    if (line.startsWith("---") || line.startsWith("+++")) continue;
    const h = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);
    if (h) {
      oldN = +h[1];
      newN = +h[2];
      rows.push({ t: "hunk", text: line });
    } else if (line.startsWith("+")) rows.push({ t: "add", n: newN++, text: line.slice(1) });
    else if (line.startsWith("-")) rows.push({ t: "del", o: oldN++, text: line.slice(1) });
    else rows.push({ t: "ctx", o: oldN++, n: newN++, text: line.slice(1) });
  }
  const shown = max ? rows.slice(0, max) : rows;
  return (
    <div className="cw-diff">
      {shown.map((r, i) => (
        <div key={i} className={clsx("cw-diff-row", r.t)} style={{ animationDelay: `${Math.min(i, 30) * 12}ms` }}>
          <span className="ln">{r.o ?? ""}</span>
          <span className="ln">{r.n ?? ""}</span>
          <span className="sign">{r.t === "add" ? "+" : r.t === "del" ? "−" : ""}</span>
          <span className="txt">{r.text || " "}</span>
        </div>
      ))}
      {max && rows.length > max && <div className="cw-diff-more">… {rows.length - max} more lines</div>}
    </div>
  );
}

function diffStat(diff: string) {
  let a = 0;
  let d = 0;
  for (const l of diff.split("\n")) {
    if (l.startsWith("+") && !l.startsWith("+++")) a++;
    else if (l.startsWith("-") && !l.startsWith("---")) d++;
  }
  return { a, d };
}

function ToolCard({ item }: { item: Extract<Item, { kind: "tool" }> }) {
  const running = item.ok === undefined;
  const isShell = item.name === "Bash";
  const [open, setOpen] = useState<boolean | null>(null);
  const expandShell = useCodeWeb((s) => s.expandShell);
  const expanded = open ?? (isShell && running && expandShell);
  useTicker(running);
  const Icon = TOOL_ICONS[item.name] ?? Wrench;
  const task = useCode((s) => Object.values(s.tasks).find((t) => t.tool_id === item.key));
  const outRef = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (outRef.current) outRef.current.scrollTop = outRef.current.scrollHeight;
  }, [item.output]);
  const stat = item.diff ? diffStat(item.diff) : null;
  const hasBody = !!item.output || !!item.diff;

  return (
    <div className={clsx("cw-tool cw-enter", running && "running", item.ok === false && "failed")}>
      <button className="cw-tool-head" onClick={() => hasBody && setOpen(!expanded)}>
        <span className="cw-tool-icon">
          <Icon size={14} />
        </span>
        <span className="cw-tool-name">{item.name}</span>
        <span className="cw-tool-title">{item.title}</span>
        {task && (
          <span className="cw-chip">
            {task.agent ? `${task.agent} · ${task.tools ?? 0} tools` : "background"}
            {task.status === "running" ? (task.last ? ` · ${task.last}` : " · running") : ` · ${task.status}`}
          </span>
        )}
        {stat && (
          <span className="cw-stat">
            <b className="add">+{stat.a}</b> <b className="del">−{stat.d}</b>
          </span>
        )}
        {(item.ended ?? Date.now()) - item.started > 0 && <span className="cw-tool-meta">{secs((item.ended ?? Date.now()) - item.started)}</span>}
        <span className="cw-tool-status">
          {running ? <span className="cw-spinner" /> : item.ok ? <Check size={14} /> : <X size={14} />}
        </span>
        {hasBody && <ChevronRight size={14} className={clsx("cw-chev", expanded && "open")} />}
      </button>
      <Collapse open={expanded && hasBody}>
        {item.diff ? (
          <DiffView diff={item.diff} />
        ) : (
          <pre ref={outRef} className={clsx("cw-tool-out", isShell && "shell")}>
            {isShell && <span className="cw-prompt">$ {String((item.input as { command?: string })?.command ?? item.title)}</span>}
            {item.output}
          </pre>
        )}
      </Collapse>
    </div>
  );
}

function Reasoning({ item }: { item: Extract<Item, { kind: "reasoning" }> }) {
  const [open, setOpen] = useState(useCodeWeb.getState().expandThinking);
  useTicker(item.streaming);
  const took = secs((item.ended ?? Date.now()) - item.started);
  return (
    <div className="cw-reason cw-enter">
      <button className="cw-reason-head" onClick={() => setOpen(!open)}>
        <Brain size={14} />
        <span className={clsx(item.streaming && "cw-shimmer")}>{item.streaming ? "Thinking" : `Thought for ${took}`}</span>
        <ChevronRight size={13} className={clsx("cw-chev", open && "open")} />
      </button>
      <Collapse open={open || item.streaming}>
        <div className="cw-reason-body">{item.text}</div>
      </Collapse>
    </div>
  );
}

export function Transcript({ dark }: { dark: boolean }) {
  const items = useCode((s) => s.items);
  const busy = useCode((s) => s.busy);
  const end = useRef<HTMLDivElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    if (stick.current) end.current?.scrollIntoView({ block: "end" });
  }, [items]);

  return (
    <div
      className="cw-scroll"
      ref={scroller}
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
      }}
    >
      <div className="cw-transcript">
        {items.map((it) => {
          switch (it.kind) {
            case "user":
              return (
                <div key={it.key} className="cw-user cw-enter">
                  <div className="cw-user-bubble">{it.text}</div>
                </div>
              );
            case "text":
              return (
                <div key={it.key} className={clsx("cw-assistant cw-enter", it.streaming && "streaming")}>
                  <Markdown text={it.text} dark={dark} />
                </div>
              );
            case "reasoning":
              return <Reasoning key={it.key} item={it} />;
            case "tool":
              return <ToolCard key={it.key} item={it} />;
            case "notice":
              return (
                <div key={it.key} className={clsx("cw-notice cw-enter", it.tone)}>
                  {it.tone === "error" ? <TriangleAlert size={13} /> : <Info size={13} />}
                  {it.text}
                </div>
              );
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
        <div ref={end} style={{ height: 1 }} />
      </div>
    </div>
  );
}
