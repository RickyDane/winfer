import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { ago, go, haptic, useClient, useTick, vtName } from "../hooks";
import { isFreeText, type Message, type Pane, type RequestOption } from "../protocol";
import { Avatar, ConnNotice, Icon, ProviderMark, STATUS_LABEL, providerInfo, toolIcon, toolText } from "./common";
import { Markdown } from "./Markdown";

type Block = { kind: "msg"; msg: Message } | { kind: "tools"; msgs: Message[] };

/** Fold runs of tool calls into one timeline so the chat stays readable. */
function blocks(messages: Message[]): Block[] {
  const out: Block[] = [];
  for (const msg of messages) {
    const last = out[out.length - 1];
    if (msg.role === "tool") {
      if (last?.kind === "tools") last.msgs.push(msg);
      else out.push({ kind: "tools", msgs: [msg] });
    } else out.push({ kind: "msg", msg });
  }
  return out;
}

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

function Steps({ msgs, active, legacyLive, fresh }: { msgs: Message[]; active: boolean; legacyLive: boolean; fresh: boolean }) {
  // Sessions that don't report states: the last call of the last group runs while the agent works.
  const states = msgs.map((m, i) => (m.state ? toolState(m, active) : legacyLive && i === msgs.length - 1 ? "running" : "unknown"));
  const [all, setAll] = useState(false);
  // Fold by count, not by state, so a run doesn't jump when its last step finishes.
  const hidden = !all && msgs.length > FOLD + 1 ? msgs.length - FOLD : 0;
  const names = [...new Set(msgs.slice(0, hidden).map((m) => m.tool ?? "tool"))];
  return (
    <div className={`steps${fresh ? " fresh" : ""}`}>
      {hidden > 0 && (
        <button className="step more" onClick={() => (haptic(5), setAll(true))}>
          <span className="step-icon">
            <Icon name="chevron" size={12} />
          </span>
          <span className="step-name">{hidden} earlier steps</span>
          <span className="step-arg">{names.slice(0, 3).join(", ")}</span>
        </button>
      )}
      {msgs.slice(hidden).map((m, i) => (
        <Step key={m.id} m={m} state={states[hidden + i]} />
      ))}
    </div>
  );
}

/** One tool call on the rail; tapping it unclamps the argument. */
function Step({ m, state }: { m: Message; state: ToolState }) {
  const [open, setOpen] = useState(false);
  return (
    <button className="step" data-state={state} data-open={open} aria-expanded={open} onClick={() => setOpen(!open)}>
      <span className="step-icon">
        <Icon name={toolIcon(m.tool)} size={12} />
      </span>
      <span className="step-name">{m.tool ?? "Tool"}</span>
      <span className="step-arg">{toolText(m.tool, m.text)}</span>
      {m.background && <span className="step-tag">bg</span>}
      <span className="step-status" aria-label={state}>
        {state === "running" ? (
          <span className="spinner" />
        ) : state === "done" ? (
          <Icon name="check" size={14} />
        ) : state === "failed" ? (
          <Icon name="x" size={14} />
        ) : state === "stopped" ? (
          <Icon name="minus" size={14} />
        ) : null}
      </span>
    </button>
  );
}

const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(s / 60);
  return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : `${m}:${String(s % 60).padStart(2, "0")}`;
};

/** Above the composer while the agent works: an orb, "Working", the time and the call in flight. */
function StatusLine({ pane, current, thinking }: { pane: Pane; current: Message | null; thinking: boolean }) {
  useTick(1000);
  const what = current ? `${current.tool ?? "Tool"} ${toolText(current.tool, current.text)}` : pane.message;
  return (
    <div className="status-line" role="status">
      <span className="orb" aria-hidden />
      <span className="shimmer">{thinking ? "Thinking" : "Working"}</span>
      <span className="sep">·</span>
      <span className="num">{clock(Date.now() - (current?.ts ?? pane.updatedAt))}</span>
      {what && (
        <>
          <span className="sep">·</span>
          <span className="status-what">{what}</span>
        </>
      )}
    </div>
  );
}

/** Background shells and subagents still running, whatever the turn is doing. */
function Background({ items }: { items: Message[] }) {
  useTick(1000);
  return (
    <div className="bg-box" role="status">
      <div className="bg-head">
        <span className="spinner" aria-hidden />
        <b>Background</b>
        <span>{items.length} running</span>
      </div>
      <ul>
        {items.map((m) => (
          <li key={m.id}>
            <Icon name={toolIcon(m.tool)} size={13} />
            <b>{m.tool ?? "Task"}</b>
            <span className="bg-text">{toolText(m.tool, m.text)}</span>
            {m.ts && <span className="num">{clock(Date.now() - m.ts)}</span>}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Where the turn ended, as a quiet divider at the bottom of the chat. */
function EndMark({ pane }: { pane: Pane }) {
  useTick(30_000);
  const when = ago(pane.updatedAt) === "now" ? "just now" : `${ago(pane.updatedAt)} ago`;
  return (
    <div className="end-mark" data-status={pane.status}>
      <Icon name={pane.status === "done" ? "check" : "x"} size={12} />
      {pane.status === "done" ? "Finished" : "Stopped with an error"} · {when}
    </div>
  );
}

const QUICK_DONE = ["Continue", "Run the tests", "Commit this", "Summarize the changes"];
const QUICK_Q = ["Yes", "No", "You decide", "Explain the options"];

export function Thread({ client, paneId }: { client: DesktopClient; paneId: string }) {
  const state = useClient(client);
  const pane = state.panes.find((p) => p.id === paneId);
  const thread = state.threads[paneId];
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [unseen, setUnseen] = useState(false);
  /** The option being answered, while it's picked but not yet sent. */
  const [picked, setPicked] = useState<number | null>(null);
  /** A "Type something" row the composer answers. */
  const [typing, setTyping] = useState<number | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const stick = useRef(true);
  const seen = useRef<Set<string> | null>(null);
  const ready = state.socket === "open" && state.desktopOnline !== false;

  const raw = thread?.messages ?? [];
  // The desktop ends the thread with a marker while the agent is reasoning.
  const thinking = raw.at(-1)?.role === "thinking";
  const messages = thinking ? raw.slice(0, -1) : raw;
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
  const all = blocks(messages);
  const background = messages.filter((m) => m.background && toolState(m, active) === "running");
  // The foreground call in flight: the newest running one that isn't background work.
  const current = working ? ([...messages].reverse().find((m) => m.role === "tool" && !m.background && m.state === "running") ?? null) : null;
  const quick = !draft && !req && pane?.status === "done" ? QUICK_DONE : !draft && req?.kind === "question" && !options.length ? QUICK_Q : null;
  const mode = working && !draft.trim() ? "stop" : draft.trim() ? "send" : "idle";

  const hint = typing !== null ? "Sent as your own answer" : working ? "Steers the agent at its next step" : req?.kind === "question" ? "Or answer in your own words" : "Typed into the agent on your desktop";

  return (
    <div className="screen">
      <header className="bar">
        <button className="icon-btn" aria-label="Back" onClick={() => go("/")}>
          <Icon name="back" />
        </button>
        {pane && <Avatar pane={pane} small morph />}
        <div className="bar-title">
          <b style={{ viewTransitionName: vtName("tt", paneId) } as CSSProperties}>{pane?.title ?? "Agent"}</b>
          <span>{pane ? [pane.workspace, info?.name].filter(Boolean).join(" · ") : state.panes.length ? "This agent's pane was closed" : "Loading…"}</span>
        </div>
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
          <ConnNotice state={state} onRetry={() => client.reconnect()} />
          {!thread && ready && (
            <div className="loading" aria-label="Loading">
              <span className="spinner" /> Loading the thread…
            </div>
          )}
          {thread && messages.length === 0 && pane?.reply && (
            <div className="turn-assistant">
              <Markdown text={pane.reply} />
            </div>
          )}
          {thread && messages.length === 0 && !pane?.reply && (
            <div className="empty">
              <span className="empty-mark">
                <img src="./icons/logo.svg" alt="" />
              </span>
              <h3>A fresh start</h3>
              <p className="sub">Send a message and it's typed into the agent on your desktop.</p>
            </div>
          )}
          {all.map((b, i) => {
            if (b.kind === "tools")
              return <Steps key={`t-${b.msgs[0].id}`} msgs={b.msgs} active={active} legacyLive={working && i === all.length - 1} fresh={isFresh(b.msgs[0].id)} />;
            const m = b.msg;
            const fresh = isFresh(m.id) ? " fresh" : "";
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
          {working && thinking && (
            <div className="steps">
              <div className="step reason">
                <span className="step-icon">
                  <Icon name="sparkle" size={12} />
                </span>
                <span className="shimmer">Thinking</span>
              </div>
            </div>
          )}
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
          <div className="request" key={`a-${req.text}`}>
            <div className="request-head">
              <Icon name="shield" size={18} />
              <div>
                <b>Permission request</b>
                <small>{info?.name ?? "The agent"} wants to run this</small>
              </div>
            </div>
            <pre className="request-code">{req.text}</pre>
            {extra.length > 1 && (
              <div className="options">
                {extra.slice(1).map((o) => (
                  <button key={o.n} className="option" data-picked={picked === o.n} disabled={!ready || picked !== null} onClick={() => void choose(o)}>
                    <span className="opt-n">{o.n}</span>
                    <span className="opt-body">
                      <b>{o.label}</b>
                    </span>
                    {picked === o.n && <span className="spinner" />}
                  </button>
                ))}
              </div>
            )}
            <div className="actions">
              <button className="btn primary" disabled={!ready || !!busy || picked !== null} onClick={() => void act("approve").catch(() => {})}>
                {busy === "approve" ? <span className="spinner" /> : <Icon name="check" size={15} />} Approve
              </button>
              {extra[0] && (
                <button className="btn" disabled={!ready || !!busy || picked !== null} onClick={() => void choose(extra[0])}>
                  {picked === extra[0].n && <span className="spinner" />}
                  <span className="btn-text">{extra[0].label}</span>
                </button>
              )}
              <button className="btn ghost danger" disabled={!ready || !!busy || picked !== null} onClick={() => void act("decline").catch(() => {})}>
                Deny
              </button>
            </div>
          </div>
        )}
        {req?.kind === "question" && (
          <div className="request" key={`q-${req.text}`}>
            <div className="request-head">
              <Icon name="question" size={18} />
              <div>
                <b>{req.text}</b>
                <small>{info?.name ?? "The agent"} asks</small>
              </div>
            </div>
            {options.length > 0 && (
              <div className="options" data-busy={picked !== null}>
                {options.filter((o) => !isFreeText(o)).map((o) => (
                  <button key={o.n} className="option" data-picked={picked === o.n} disabled={!ready || (picked !== null && picked !== o.n)} onClick={() => void choose(o)}>
                    <span className="opt-n">{o.n}</span>
                    <span className="opt-body">
                      <b>{o.label}</b>
                      {o.detail && <small>{o.detail}</small>}
                    </span>
                    {picked === o.n && <span className="spinner" />}
                  </button>
                ))}
                {options.filter(isFreeText).map((o) => (
                  <button key={o.n} className="option free" data-picked={typing === o.n} disabled={!ready || picked !== null} onClick={() => void choose(o)}>
                    <span className="opt-n">
                      <Icon name="pencil" size={11} />
                    </span>
                    <span className="opt-body">
                      <b>{typing === o.n ? "Type your answer below" : "Or type your own answer"}</b>
                    </span>
                  </button>
                ))}
              </div>
            )}
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
        {pane && working && <StatusLine pane={pane} current={current} thinking={thinking} />}
        <div className="composer" data-busy={working || undefined}>
          <textarea
            ref={input}
            rows={1}
            value={draft}
            enterKeyHint="send"
            placeholder={typingLabel ? `${typingLabel.replace(/\.$/, "")}…` : req?.kind === "question" ? "Type your answer…" : working ? "Steer the agent…" : "Message the agent…"}
            onChange={(e) => {
              setDraft(e.target.value);
              e.target.style.height = "auto";
              e.target.style.height = `${Math.min(e.target.scrollHeight, 140)}px`;
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void act("send").catch(() => {});
            }}
          />
          <div className="composer-bar">
            <span className="hint">{hint}</span>
            <button
              className="send"
              data-mode={mode}
              aria-label={mode === "stop" ? "Interrupt" : "Send"}
              disabled={!ready || !!busy || mode === "idle"}
              onClick={() => void act(mode === "stop" ? "interrupt" : "send").catch(() => {})}
            >
              <Icon name={mode === "stop" ? "stop" : "send"} size={mode === "stop" ? 18 : 16} />
            </button>
          </div>
        </div>
        {pane && (
          <div className="foot">
            <span className="foot-item mode" data-status={pane.status}>
              {STATUS_LABEL[pane.status]}
            </span>
            {info && (
              <span className="foot-item prov" style={{ color: info.mark }}>
                <ProviderMark provider={pane.provider} size={12} />
                <span>{info.name}</span>
              </span>
            )}
            <span className="foot-item dim">
              <Icon name="folder" size={12} />
              <span>{pane.workspace}</span>
            </span>
            <span className="grow" />
            <span className="foot-item dim">{ago(pane.updatedAt)}</span>
          </div>
        )}
      </footer>
    </div>
  );
}
