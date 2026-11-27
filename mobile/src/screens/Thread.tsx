import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { ago, firstMessages, fromTask, go, haptic, useClient, useTick, vtName } from "../hooks";
import { isFreeText, type BgItem, type Message, type Pane, type RequestOption } from "../protocol";
import { Composer, Pill, PillIcon } from "./Composer";
import { activeIsServer } from "../pairing";
import { forMachine } from "../multi";
import { Avatar, ConnNotice, Icon, ProviderMark, STATUS_LABEL, providerInfo, toolIcon, toolText, toolTint, type IconName } from "./common";
import { VizReport } from "./Viz";
import { buildDisplayRows, describeStep, useCompactTools, workGroupSummary } from "./compactTools";
import { useShowBgNotes } from "./bgNotes";
import { Markdown } from "./Markdown";
import { EffortIcon, EffortPicker, ModelPicker, Sheet, backendOf, effortLabel } from "./Pickers";
import { PanelSheet, hasPanel } from "./Panel";
import { Sky } from "./Sky";

type ToolState = "running" | "done" | "failed" | "stopped" | "unknown";

/**
 * A tool call's state as the phone should show it. Calls still "running" in a session that is no
 * longer active were cut off (the agent was interrupted); background work outlives its turn.
 */
function toolState(m: Message, active: boolean): ToolState {
  if (m.state === "running") return m.background || active ? "running" : "stopped";
  return m.state ?? "unknown";
}

/** Runs longer than this fold their earlier steps behind one row. */
const FOLD = 5;

const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(s / 60);
  return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : m > 0 ? `${m}m ${s % 60}s` : `${s}s`;
};

/** Consecutive tool calls as the desktop's chat shows them: cards whose icons a line joins. */
/** A finished background task: one line; tap for its command and output. */
function BgRow({ msg, fresh }: { msg: Message; fresh: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <div className={`bg-row${fresh ? " fresh" : ""}`} data-state={msg.state ?? "done"} data-open={open}>
      <button onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className="bg-dot" />
        <span className="bg-title">{msg.text}</span>
        <Icon name="right" size={14} className="bg-chev" />
      </button>
      {open && msg.detail && <pre className="bg-detail">{msg.detail}</pre>}
    </div>
  );
}

const NOTICE_ICON: Record<NonNullable<Message["kind"]>, IconName> = { compact: "layers", microcompact: "minus", account: "refresh" };

/** A session note the desktop shows too: compaction, micro-compaction, an account change. */
function NoticeRow({ msg, fresh }: { msg: Message; fresh: boolean }) {
  return (
    <div className={`thread-notice${fresh ? " fresh" : ""}`} data-kind={msg.kind} role="note">
      <span>
        <Icon name={NOTICE_ICON[msg.kind ?? "compact"] ?? "info"} size={12} />
        {msg.text}
      </span>
    </div>
  );
}

function Steps({ msgs, active, legacyLive, fresh }: { msgs: Message[]; active: boolean; legacyLive: boolean; fresh: boolean }) {
  // Sessions that don't report states: the last call of the last group runs while the agent works.
  const states = msgs.map((m, i) => (m.state ? toolState(m, active) : legacyLive && i === msgs.length - 1 ? "running" : "unknown"));
  const [all, setAll] = useState(false);
  // Fold by count, not by state, so a run doesn't jump when its last step finishes.
  const hidden = !all && msgs.length > FOLD + 1 ? msgs.length - FOLD : 0;
  const names = [...new Set(msgs.slice(0, hidden).map((m) => m.tool ?? "tool"))];
  const rows = msgs.slice(hidden);
  return (
    <div className={`tools${fresh ? " fresh" : ""}`}>
      {hidden > 0 && (
        <div className="tool-wrap" data-down>
          <button className="tool more" onClick={() => (haptic(5), setAll(true))}>
            <span className="tool-badge">
              <Icon name="chevron" size={13} />
            </span>
            <span className="tool-name">{hidden} earlier steps</span>
            <span className="tool-title">{names.slice(0, 3).join(", ")}</span>
          </button>
        </div>
      )}
      {rows.map((m, i) => (
        <ToolRow key={m.id} m={m} state={states[hidden + i]} up={hidden > 0 || i > 0} down={i < rows.length - 1} />
      ))}
    </div>
  );
}

/** Compacted run of tool calls — mirrors native-gpui's `work_group_row` & `WorkGroupScroll`. */
function WorkGroup({
  msgs,
  active,
  thinking,
  sessionActive,
  legacyLive,
  fresh,
}: {
  msgs: Message[];
  active: boolean;
  thinking: boolean;
  sessionActive: boolean;
  legacyLive: boolean;
  fresh: boolean;
}) {
  const [open, setOpen] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [fades, setFades] = useState({ top: 0, bot: 0 });

  const syncFades = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const max = el.scrollHeight - el.clientHeight;
    if (max <= 1) {
      setFades((f) => (f.top === 0 && f.bot === 0 ? f : { top: 0, bot: 0 }));
      return;
    }
    const top = Math.min(1, Math.max(0, el.scrollTop / 24));
    const bot = Math.min(1, Math.max(0, (max - el.scrollTop) / 24));
    setFades((f) => (Math.abs(f.top - top) < 0.02 && Math.abs(f.bot - bot) < 0.02 ? f : { top, bot }));
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    syncFades();
    const el = scrollRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(syncFades);
    ro.observe(el);
    return () => ro.disconnect();
  }, [open, msgs.length, syncFades]);

  const states = msgs.map((m, i) => (m.state ? toolState(m, sessionActive) : legacyLive && i === msgs.length - 1 ? "running" : "unknown"));
  const lastStep = msgs[msgs.length - 1] ?? null;

  return (
    <div className={`wg${fresh ? " fresh" : ""}`} data-open={open} data-active={active || undefined}>
      <button
        className="wg-head"
        aria-expanded={open}
        onClick={() => {
          haptic(5);
          setOpen((v) => !v);
        }}
      >
        {active && <span className="wg-spinner" aria-hidden />}
        <span className={active ? "wg-label wg-live" : "wg-label"}>{active ? describeStep(lastStep, thinking) : workGroupSummary(msgs)}</span>
        <Icon name="right" size={13} className="wg-chev" />
      </button>
      {open && (
        <div className="wg-body-wrap">
          <div className="wg-scroll" ref={scrollRef} onScroll={syncFades}>
            <div className="tools">
              {msgs.map((m, i) => (
                <ToolRow key={m.id} m={m} state={states[i]} up={i > 0} down={i < msgs.length - 1} />
              ))}
            </div>
          </div>
          <div className="wg-fade wg-fade-top" style={{ opacity: fades.top }} aria-hidden />
          <div className="wg-fade wg-fade-bot" style={{ opacity: fades.bot }} aria-hidden />
        </div>
      )}
    </div>
  );
}

/** One tool call: its tinted icon, name and argument, time and outcome; tap for the whole argument. */
function ToolRow({ m, state, up, down }: { m: Message; state: ToolState; up: boolean; down: boolean }) {
  const [open, setOpen] = useState(false);
  const running = state === "running";
  useTick(running ? 1000 : 0);
  const tint = state === "failed" ? "var(--danger)" : toolTint(m.tool);
  const text = toolText(m.tool, m.text);
  return (
    <div className="tool-wrap" data-up={(up && !open) || undefined} data-down={(down && !open) || undefined}>
      <div className="tool-card" data-open={open} data-state={state}>
        <button className="tool" aria-expanded={open} onClick={() => setOpen(!open)}>
          <span className="tool-badge" style={{ "--tint": tint } as CSSProperties}>
            <Icon name={toolIcon(m.tool)} size={13} />
          </span>
          <span className="tool-name">{m.tool ?? "Tool"}</span>
          <span className="tool-title">{text}</span>
          {m.background && <span className="tool-tag">bg</span>}
          {running && m.ts && <span className="tool-time">{clock(Date.now() - m.ts)}</span>}
          <span className="tool-status" aria-label={state}>
            {running ? <span className="spinner" /> : state === "done" ? <Icon name="check" size={13} /> : state === "failed" ? <Icon name="x" size={13} /> : state === "stopped" ? <Icon name="minus" size={13} /> : null}
          </span>
          <Icon name="right" size={12} className="tool-chev" />
        </button>
        {open && <pre className="tool-body">{text || "No details for this call."}</pre>}
      </div>
    </div>
  );
}

/** The desktop's working row: a spinner, "Working" (or "Thinking"), the time and the call in flight. */
function WorkingRow({ pane, current, thinking }: { pane: Pane; current: Message | null; thinking: boolean }) {
  useTick(1000);
  const what = current ? `${current.tool ?? "Tool"} ${toolText(current.tool, current.text)}` : pane.message;
  return (
    <div className="working" role="status">
      <span className="orb" aria-hidden />
      <span className="shimmer">{thinking ? "Thinking" : "Working"}</span>
      <span className="num">{clock(Date.now() - (current?.ts ?? pane.updatedAt))}</span>
      {what && <span className="working-what">{what}</span>}
    </div>
  );
}

/** Background shells and subagents still running, whatever the turn is doing. */
function Background({ items }: { items: Message[] }) {
  useTick(1000);
  return (
    <div className="bg-card" role="status">
      <div className="bg-head">
        <span className="spinner" aria-hidden />
        <b>Background</b>
        <span>{items.length} running</span>
      </div>
      <ul>
        {items.map((m) => (
          <li key={m.id}>
            <span className="tool-badge sm" style={{ "--tint": toolTint(m.tool) } as CSSProperties}>
              <Icon name={toolIcon(m.tool)} size={11} />
            </span>
            <b>{m.tool ?? "Task"}</b>
            <span className="bg-text">{toolText(m.tool, m.text)}</span>
            {m.ts && <span className="num">{clock(Date.now() - m.ts)}</span>}
          </li>
        ))}
      </ul>
    </div>
  );
}

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** The desktop header's pill: how many subagents and background shells are running; tap for the list. */
function BgChip({ items, open, onOpen }: { items: BgItem[]; open: boolean; onOpen: () => void }) {
  const agents = items.filter((t) => t.kind === "agent").length;
  const shells = items.length - agents;
  const label = [agents && plural(agents, "subagent"), shells && plural(shells, "background shell")].filter(Boolean).join(" · ");
  return (
    <button className="bg-chip" data-open={open || undefined} aria-label={`${label} running`} onClick={() => (haptic(5), onOpen())}>
      <span className="bg-chip-pulse" aria-hidden />
      {agents > 0 && (
        <span className="bg-chip-part">
          <Icon name="bot" size={12} />
          <b className="num">{agents}</b>
          <span className="bg-chip-word">{agents === 1 ? "agent" : "agents"}</span>
        </span>
      )}
      {agents > 0 && shells > 0 && <span className="bg-chip-dot" aria-hidden />}
      {shells > 0 && (
        <span className="bg-chip-part">
          <Icon name="terminal" size={12} />
          <b className="num">{shells}</b>
          <span className="bg-chip-word">{shells === 1 ? "shell" : "shells"}</span>
        </span>
      )}
    </button>
  );
}

/** The desktop's "In the background" popover: each running subagent and shell, with a stop button. */
function BgSheet({ client, paneId, items, ready, onClose }: { client: DesktopClient; paneId: string; items: BgItem[]; ready: boolean; onClose: () => void }) {
  useTick(1000);
  const [stopping, setStopping] = useState<Set<string>>(new Set());
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    if (!items.length) onClose();
  }, [items.length, onClose]);
  const stop = async (t: BgItem) => {
    if (stopping.has(t.id)) return;
    haptic([6, 30, 6]);
    setErr(null);
    setStopping((s) => new Set(s).add(t.id));
    try {
      await client.request({ t: "bg.stop", paneId, taskId: t.id });
    } catch (e) {
      setErr((e as Error).message);
      setStopping((s) => {
        const next = new Set(s);
        next.delete(t.id);
        return next;
      });
    }
  };
  const agents = items.filter((t) => t.kind === "agent").length;
  const shells = items.length - agents;
  const sub = [agents && plural(agents, "subagent"), shells && plural(shells, "shell")].filter(Boolean).join(" · ") + " running";
  return (
    <Sheet label="In the background" title="In the background" sub={sub} onClose={onClose}>
      <ul className="bg-list">
        {items.map((t) => (
          <li key={t.id} data-kind={t.kind}>
            <span className="bg-list-icon" aria-hidden>
              <Icon name={t.kind === "agent" ? "bot" : "terminal"} size={13} />
            </span>
            <div className="bg-list-body">
              <div className="bg-list-top">
                <b>{t.title}</b>
                <span className="num">{clock(Date.now() - t.startedAt)}</span>
              </div>
              <div className="bg-list-sub">
                <span className="bg-list-tag">{t.label}</span>
                <span className="bg-list-detail">{t.detail || (t.kind === "agent" ? "starting…" : "running…")}</span>
              </div>
            </div>
            <button className="bg-stop" disabled={!ready || stopping.has(t.id)} aria-label={`Stop ${t.title}`} onClick={() => stop(t)}>
              {stopping.has(t.id) ? <span className="spinner" aria-hidden /> : <Icon name="stop" size={10} />} Stop
            </button>
          </li>
        ))}
      </ul>
      {err && <p className="inline-error bg-err">{err}</p>}
    </Sheet>
  );
}

/** Where the turn ended, as the desktop's "Worked for…" divider. */
function EndMark({ pane }: { pane: Pane }) {
  useTick(30_000);
  const when = ago(pane.updatedAt) === "now" ? "just now" : `${ago(pane.updatedAt)} ago`;
  return (
    <div className="end-mark" data-status={pane.status}>
      <span>
        <Icon name={pane.status === "done" ? "check" : "alert"} size={11} />
        {pane.status === "done" ? "Finished" : "Stopped with an error"} · {when}
      </span>
    </div>
  );
}

/** The desktop's starter prompts on a fresh session. */
export const SUGGESTIONS: { icon: IconName; title: string; prompt: string }[] = [
  { icon: "compass", title: "Explore this codebase", prompt: "Give me a tour of this project: structure, entry points and how it fits together." },
  { icon: "bug", title: "Find and fix a bug", prompt: "Look for a likely bug in this project and fix it, explaining what was wrong." },
  { icon: "flask", title: "Write tests", prompt: "Find the least-tested important code here and add good tests for it." },
  { icon: "bulb", title: "Suggest improvements", prompt: "Review this project and suggest the three most valuable improvements." },
];

/** A fresh session: "What should we build?", who answers and where, and four ways to start. */
export function Welcome({ status, where, onPick, disabled, server }: { status: string; where?: string | null; onPick?: (prompt: string) => void; disabled?: boolean; server?: boolean }) {
  return (
    <div className="welcome">
      <div className="welcome-head">
        <h1>What should we build?</h1>
        <p>{status}</p>
        {where && <code className="welcome-where">{where}</code>}
      </div>
      {onPick && (
        <div className="suggest">
          {SUGGESTIONS.map((sg, i) => (
            <button key={sg.title} className="suggest-card" style={{ animationDelay: `${0.22 + i * 0.07}s` }} disabled={disabled} onClick={() => onPick(sg.prompt)}>
              <span className="suggest-icon">
                <Icon name={sg.icon} size={14} />
              </span>
              <span>{sg.title}</span>
            </button>
          ))}
        </div>
      )}
      <p className="welcome-hint">Type a message below, and it's sent to the agent on your {(server ?? activeIsServer()) ? "server" : "desktop"}</p>
    </div>
  );
}

const QUICK_DONE = ["Continue", "Run the tests", "Commit this", "Summarize the changes"];
const QUICK_Q = ["Yes", "No", "You decide", "Explain the options"];

/** `/Users/me/code/app` → `~/code/app`. */
export const homePath = (p: string) => p.replace(/^\/(Users|home)\/[^/]+/, "~");

export function Thread({ client, paneId }: { client: DesktopClient; paneId: string }) {
  const state = useClient(client);
  const pane = state.panes.find((p) => p.id === paneId);
  const thread = state.threads[paneId];
  // This pane's own machine's link, when several are paired.
  const link = forMachine(state, pane?.machine);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [unseen, setUnseen] = useState(false);
  /** The option being answered, while it's picked but not yet sent. */
  const [picked, setPicked] = useState<number | null>(null);
  /** A "Type something" row the composer answers. */
  const [typing, setTyping] = useState<number | null>(null);
  const [picker, setPicker] = useState<"model" | "effort" | "menu" | "bg" | "panel" | null>(null);
  const [confirm, setConfirm] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const stick = useRef(true);
  const seen = useRef<Set<string> | null>(null);
  const ready = link.socket === "open" && link.desktopOnline !== false;

  const raw = thread?.messages ?? [];
  // The desktop ends the thread with a marker while the agent is reasoning.
  const thinking = raw.at(-1)?.role === "thinking";
  const showBgNotes = useShowBgNotes(state.showBgNotes);
  const unthought = thinking ? raw.slice(0, -1) : raw;
  const messages = showBgNotes ? unthought : unthought.filter((m) => m.role !== "bg");
  const bgRunning = messages.some((m) => m.background && m.state === "running");

  // Live winfer sessions push updates; reads renew the lease and heal reconnects.
  // Other providers' session files are polled independently of turn completion.
  const statusKey = pane ? `${pane.status}:${pane.updatedAt}` : "";
  useEffect(() => {
    if (!ready) return;
    let loading = false;
    const load = async () => {
      if (loading || document.visibilityState === "hidden") return;
      loading = true;
      try { await client.request({ t: "thread.get", paneId, limit: 200 }); } catch { /* retry on the next tick */ }
      finally { loading = false; }
    };
    load();
    const every = pane?.provider === "winfer" ? 15_000 : pane?.status === "working" || bgRunning ? 1000 : 15_000;
    const t = setInterval(load, every);
    const visible = () => { if (document.visibilityState === "visible") void load(); };
    document.addEventListener("visibilitychange", visible);
    return () => { clearInterval(t); document.removeEventListener("visibilitychange", visible); };
  }, [client, paneId, ready, statusKey, pane?.status, bgRunning]);

  // Only messages that arrive after the first load animate in.
  const isFresh = (id: string) => !!seen.current && !seen.current.has(id);
  useEffect(() => {
    if (!thread) return;
    const ids = new Set(messages.map((m) => m.id));
    if (seen.current) for (const id of ids) seen.current.add(id);
    else seen.current = ids;
  });

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    if (stick.current) el.scrollTo({ top: el.scrollHeight, behavior: seen.current ? "smooth" : "auto" });
    else setUnseen(true);
  }, [messages.length, raw.at(-1)?.id, raw.at(-1)?.text, pane?.request, pane?.status]);

  async function act(kind: "send" | "approve" | "decline" | "interrupt", text = draft.trim()) {
    if (busy) return;
    if (kind === "send" && !text) return;
    setBusy(kind);
    setErr(null);
    haptic(kind === "interrupt" ? [6, 30, 6] : 8);
    stick.current = true;
    if (kind === "send") {
      setDraft("");
      if (input.current) input.current.style.height = "";
    }
    try {
      // Text for a "Type something" row answers through that row.
      const msg =
        kind === "send" && typing !== null ? ({ t: "choose", paneId, option: typing, text } as const) : kind === "send" ? ({ t: "send", paneId, text } as const) : ({ t: kind, paneId } as const);
      if (msg.t === "send" && !pane?.request) await client.sendMessage(paneId, text);
      else await client.request(msg);
      if (kind === "send") {
        setTyping(null);
      }
      stick.current = true;
    } catch (e) {
      if (kind === "send") setDraft((current) => current || text);
      setErr((e as Error).message);
      throw e;
    } finally {
      setBusy(null);
    }
  }

  async function choose(o: RequestOption) {
    if (busy || picked !== null) return;
    if (isFreeText(o)) {
      haptic(5);
      setTyping(o.n);
      input.current?.focus();
      return;
    }
    setPicked(o.n);
    setErr(null);
    haptic([8, 30, 8]);
    try {
      await client.request({ t: "choose", paneId, option: o.n });
      stick.current = true;
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      // The next request (or status) replaces this sheet; drop the pick after a beat either way.
      setTimeout(() => setPicked(null), 900);
    }
  }

  const screen = thread?.source === "screen";
  const req = pane?.request;
  const options = req?.options ?? [];
  // Approve / decline cover the first and the "No" rows; offer the rest ("Yes, and don't ask again").
  const extra = req?.kind === "approval" ? options.filter((o) => o.n !== 1 && !/^no\b/i.test(o.label)) : [];
  const typingLabel = typing !== null ? options.find((o) => o.n === typing)?.label : undefined;
  const working = pane?.status === "working";
  const info = providerInfo(pane?.provider ?? null);
  const active = working || pane?.status === "needs-input";
  const compactTools = useCompactTools(state.compactTools);
  const { rows: displayRows, hasLiveGroup } = buildDisplayRows(messages, working, thinking, compactTools);
  const background = messages.filter((m) => m.background && toolState(m, active) === "running");
  // The foreground call in flight: the newest running one that isn't background work.
  const current = working ? ([...messages].reverse().find((m) => m.role === "tool" && !m.background && m.state === "running") ?? null) : null;
  const quick = !draft && !req && pane?.status === "done" ? QUICK_DONE : !draft && req?.kind === "question" && !options.length ? QUICK_Q : null;
  const mode = working && !draft.trim() ? "stop" : draft.trim() ? "send" : "idle";


  const cfg = pane?.config ?? null;
  const backend = cfg ? backendOf(cfg.model) : null;
  // The message this session was started with, until the thread has it.
  const first = messages.length === 0 && !pane?.reply ? (firstMessages.get(paneId) ?? null) : null;
  useEffect(() => {
    if (messages.length > 0) firstMessages.delete(paneId);
  }, [paneId, messages.length]);
  const empty = !!thread && messages.length === 0 && !pane?.reply && !first;
  const where = cfg?.cwd ? `${homePath(cfg.cwd)}${cfg.branch ? `  ⎇ ${cfg.branch}` : ""}` : null;
  const welcomeStatus = [cfg?.label || info?.name, backend?.name !== cfg?.label ? backend?.name : null, cfg?.mode].filter(Boolean).join(" · ") || "Ready";

  const sendPrompt = (text: string) => {
    if (!ready || busy) return;
    haptic(8);
    stick.current = true;
    setErr(null);
    setBusy("send");
    client
      .sendMessage(paneId, text)
      .catch((e: Error) => setErr(e.message))
      .finally(() => setBusy(null));
  };

  const closePicker = useCallback(() => setPicker(null), []);

  // Back leads to the tab that lists this pane, also after it's gone.
  const fromHome = useRef(false);
  if (pane) fromHome.current = !!pane.home;
  const back = () => go(fromHome.current || fromTask(paneId) ? "/home" : "/");

  const closeSession = () => {
    if (!pane) return;
    if (pane.status === "working" && !confirm) return setConfirm(true);
    haptic([10, 30, 10]);
    setPicker(null);
    void client.request({ t: "pane.close", paneId: pane.id }).catch(() => {});
    back();
  };

  const pills = pane ? (
    cfg ? (
      <>
        {cfg.mode && <Pill icon={<PillIcon name={cfg.yolo ? "bolt" : "shield"} />} label={cfg.mode} tone={cfg.yolo ? "warn" : undefined} title={`Mode: ${cfg.mode}`} />}
        <Pill
          icon={backend ? <ProviderMark provider={backend.id} size={14} /> : <PillIcon name="sparkle" />}
          label={cfg.label || "Model"}
          title={`Model: ${cfg.label || "default"}`}
          disabled={!ready}
          onClick={() => (haptic(5), setPicker("model"))}
        />
        {cfg.efforts.length > 1 && (
          <Pill icon={<EffortIcon effort={cfg.effort} />} label={cfg.effort === "default" ? "default" : cfg.effort} title={`Reasoning effort: ${effortLabel(cfg.effort)}`} disabled={!ready} onClick={() => (haptic(5), setPicker("effort"))} />
        )}
        {cfg.branch && <Pill icon={<PillIcon name="branch" />} label={cfg.branch} title={`Branch: ${cfg.branch}`} />}
      </>
    ) : (
      info && <Pill icon={<span style={{ color: info.mark, display: "grid" }}><ProviderMark provider={pane.provider} size={14} /></span>} label={info.name} />
    )
  ) : null;

  return (
    <div className="screen thread" data-empty={empty || undefined}>
      {empty && <Sky />}
      <header className="pane-head">
        <button className="icon-btn" aria-label="Back" onClick={back}>
          <Icon name="back" size={19} />
        </button>
        {pane && <Avatar pane={pane} small morph />}
        <div className="pane-title">
          <b style={{ viewTransitionName: vtName("tt", paneId) } as CSSProperties}>{pane?.title ?? "Agent"}</b>
          <span className="pane-meta">
            {pane ? (
              <>
                {pane.machineName && <span className="meta-item">{pane.machineName}</span>}
                <span className="chip-ws">{pane.workspace}</span>
                {cfg?.branch ? (
                  <span className="meta-item">
                    <Icon name="branch" size={11} /> {cfg.branch}
                  </span>
                ) : (
                  info && <span className="meta-item">{info.name}</span>
                )}
                <span className="meta-status" data-status={pane.status}>
                  {STATUS_LABEL[pane.status]}
                </span>
              </>
            ) : state.panes.length ? (
              "This agent's pane was closed"
            ) : (
              "Loading…"
            )}
          </span>
        </div>
        {pane?.background?.length ? <BgChip items={pane.background} open={picker === "bg"} onOpen={() => setPicker("bg")} /> : null}
        {pane && hasPanel(state, pane) && (
          <button className="icon-btn panel-btn" aria-label="Side panel" title="Side panel" data-on={picker === "panel" || undefined} onClick={() => (haptic(5), setPicker("panel"))}>
            <Icon name="panel" size={18} />
          </button>
        )}
        {pane && (
          <button className="icon-btn" aria-label="More" onClick={() => (haptic(5), setConfirm(false), setPicker("menu"))}>
            <Icon name="more" size={18} />
          </button>
        )}
      </header>

      <div
        className="scroll"
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
          if (stick.current) setUnseen(false);
        }}
      >
        <div className="transcript">
          <ConnNotice state={link} onRetry={() => client.reconnect()} />
          {first && (
            <div className="turn-user">
              <p>{first}</p>
            </div>
          )}
          {!thread && ready && !first && (
            <div className="loading" aria-label="Loading">
              <span className="spinner" /> Loading the thread…
            </div>
          )}
          {thread && messages.length === 0 && pane?.reply && (
            <div className="turn-assistant">
              <Markdown text={pane.reply} />
            </div>
          )}
          {empty && <Welcome status={welcomeStatus} where={where} server={link.welcome?.server} disabled={!ready || !!busy} onPick={pane?.request ? undefined : sendPrompt} />}
          {displayRows.map((b, i) => {
            if (b.kind === "work-group")
              return (
                <WorkGroup
                  key={b.key}
                  msgs={b.msgs}
                  active={b.active}
                  thinking={b.thinking}
                  sessionActive={active}
                  legacyLive={working && i === displayRows.length - 1}
                  fresh={isFresh(b.msgs[0].id)}
                />
              );
            if (b.kind === "tools")
              return <Steps key={b.key} msgs={b.msgs} active={active} legacyLive={working && i === displayRows.length - 1} fresh={isFresh(b.msgs[0].id)} />;
            const m = b.msg;
            const fresh = isFresh(m.id) ? " fresh" : "";
            if (m.report) return <div key={m.id} className={`turn-viz${fresh}`}><VizReport report={m.report} /></div>;
            if (m.role === "bg") return <BgRow key={m.id} msg={m} fresh={fresh !== ""} />;
            if (m.role === "notice") return <NoticeRow key={m.id} msg={m} fresh={fresh !== ""} />;
            if (m.role === "user")
              return (
                <div key={m.id} className={`turn-user${fresh}`}>
                  <p>{m.text}</p>
                  {m.ts && <span className="time">{ago(m.ts)}</span>}
                </div>
              );
            return (
              <div key={m.id} className={`turn-assistant${fresh}`}>
                {screen ? <pre className="screen-text">{m.text}</pre> : <Markdown text={m.text} />}
              </div>
            );
          })}
          {first && !(pane && working) && (
            <div className="loading" aria-label="Starting">
              <span className="spinner" /> Starting {info?.name ?? "the agent"}…
            </div>
          )}
          {pane && working && !hasLiveGroup && <WorkingRow pane={pane} current={current} thinking={thinking} />}
          {pane && background.length > 0 && <Background items={background} />}
          {pane && !working && (pane.status === "done" || pane.status === "error") && messages.length > 0 && <EndMark pane={pane} />}
        </div>
      </div>

      <footer className="dock">
        {unseen && (
          <button
            className="jump"
            aria-label="Jump to latest"
            onClick={() => {
              stick.current = true;
              setUnseen(false);
              scroller.current?.scrollTo({ top: scroller.current.scrollHeight, behavior: "smooth" });
            }}
          >
            <Icon name="down" size={16} />
          </button>
        )}
        {req?.kind === "approval" && (
          <div className="ask" key={`a-${req.text}`}>
            <div className="ask-head">
              <Icon name="shield" size={15} />
              <span>{info?.name ?? "The agent"} wants to run</span>
            </div>
            <div className="ask-scroll">
              <pre className="ask-code">{req.text}</pre>
            </div>
            <div className="ask-actions">
              <button className="btn primary" disabled={!ready || !!busy || picked !== null} onClick={() => void act("approve").catch(() => {})}>
                {busy === "approve" && <span className="spinner" />} Allow
              </button>
              {extra[0] && (
                <button className="btn" disabled={!ready || !!busy || picked !== null} onClick={() => void choose(extra[0])}>
                  {picked === extra[0].n && <span className="spinner" />}
                  <span className="btn-text">{extra[0].label}</span>
                </button>
              )}
              <button className="btn" disabled={!ready || !!busy || picked !== null} onClick={() => void act("decline").catch(() => {})}>
                Deny
              </button>
            </div>
            {extra.length > 1 && (
              <div className="ask-more">
                {extra.slice(1).map((o) => (
                  <button key={o.n} className="choice" data-picked={picked === o.n} disabled={!ready || picked !== null} onClick={() => void choose(o)}>
                    <b>{o.label}</b>
                    {picked === o.n && <span className="spinner" />}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
        {req?.kind === "question" && (
          <div className="ask" key={`q-${req.text}`}>
            <div className="ask-head">
              <Icon name="shield" size={15} />
              <span>{info?.name ?? "The agent"} is asking</span>
            </div>
            <div className="ask-scroll">
              <QuestionText text={req.text} />
              {options.length > 0 && (
                <div className="choices" data-busy={picked !== null}>
                  {options.filter((o) => !isFreeText(o)).map((o) => (
                    <button key={o.n} className="choice" data-picked={picked === o.n} disabled={!ready || (picked !== null && picked !== o.n)} onClick={() => void choose(o)}>
                      <span className="choice-body">
                        <b>{o.label}</b>
                        {o.detail && <small>{o.detail}</small>}
                      </span>
                      {picked === o.n && <span className="spinner" />}
                    </button>
                  ))}
                </div>
              )}
            </div>
            <div className="ask-foot">
              {options.filter(isFreeText).map((o) => (
                <button key={o.n} className="choice-free" data-on={typing === o.n} disabled={!ready || picked !== null} onClick={() => void choose(o)}>
                  <Icon name="pencil" size={12} />
                  {typing === o.n ? "Type your answer below" : "or type an answer below"}
                </button>
              ))}
              {!options.some(isFreeText) && <span className="ask-hint">or type an answer below</span>}
            </div>
          </div>
        )}
        {err && <div className="inline-error">{err}</div>}
        {quick && ready && (
          <div className="chips">
            {quick.map((q) => (
              <button
                key={q}
                className="chip"
                onClick={() => {
                  setDraft(q);
                  haptic(5);
                  input.current?.focus();
                }}
              >
                {q}
              </button>
            ))}
          </div>
        )}
        <Composer
          inputRef={input}
          value={draft}
          onChange={setDraft}
          busy={working}
          mode={mode}
          disabled={!ready || !!busy}
          onSend={() => void act(mode === "stop" ? "interrupt" : "send").catch(() => {})}
          placeholder={typingLabel ? `${typingLabel.replace(/\.$/, "")}…` : req?.kind === "question" ? "Type your answer…" : working ? "Steer the agent…" : `Message ${info?.name ?? "the agent"}…`}
          pills={pills}
        />
      </footer>
      {picker === "model" && pane && cfg && <ModelPicker client={client} pane={pane} config={cfg} online={ready} onClose={() => setPicker(null)} />}
      {picker === "effort" && pane && cfg && <EffortPicker client={client} pane={pane} config={cfg} online={ready} onClose={() => setPicker(null)} />}
      {picker === "bg" && pane && <BgSheet client={client} paneId={pane.id} items={pane.background ?? []} ready={ready} onClose={closePicker} />}
      {picker === "panel" && pane && <PanelSheet client={client} pane={pane} onClose={() => setPicker(null)} />}
      {picker === "menu" && pane && (
        <Sheet label="Agent" title={pane.title} sub={`${pane.workspace} · ${STATUS_LABEL[pane.status]}`} onClose={() => setPicker(null)}>
          <div className="sheet-body">
            {cfg && (
              <>
                <button className="menu-item" onClick={() => setPicker("model")}>
                  {backend ? <ProviderMark provider={backend.id} size={16} /> : <Icon name="sparkle" size={16} />} Model
                  <span className="menu-value">{cfg.label}</span>
                </button>
                {cfg.efforts.length > 1 && (
                  <button className="menu-item" onClick={() => setPicker("effort")}>
                    <EffortIcon effort={cfg.effort} size={16} /> Reasoning effort
                    <span className="menu-value">{effortLabel(cfg.effort)}</span>
                  </button>
                )}
              </>
            )}
            {hasPanel(state, pane) && (
              <button className="menu-item" onClick={() => setPicker("panel")}>
                <Icon name="panel" size={16} /> Side panel
                <span className="menu-value">Browser, terminal, files</span>
              </button>
            )}
            <button className="menu-item danger" disabled={!ready} onClick={closeSession}>
              <Icon name="x" size={16} /> {confirm ? "Stop the agent and close?" : "Close session"}
            </button>
          </div>
        </Sheet>
      )}
    </div>
  );
}

/** A question's text: its header line (a form's "Style (1/2)") small above the question. */
function QuestionText({ text }: { text: string }) {
  const lines = text.split("\n");
  const header = lines.length > 1 && lines[0].length < 60 && !/\?\s*$/.test(lines[0]) ? lines[0] : null;
  const body = header ? lines.slice(1).join("\n") : text;
  return (
    <div className="ask-q">
      {header && <span className="ask-label">{header}</span>}
      <p>{body}</p>
    </div>
  );
}
