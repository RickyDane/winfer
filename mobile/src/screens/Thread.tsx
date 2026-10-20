import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { ago, go, haptic, useClient, useTick, vtName } from "../hooks";
import { isFreeText, type Message, type Pane, type RequestOption } from "../protocol";
import { Avatar, ConnNotice, Icon, providerInfo, toolIcon, toolText } from "./common";
import { Markdown } from "./Markdown";
import { SlideToConfirm } from "./widgets";

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

function Tools({ msgs, active, legacyLive, fresh }: { msgs: Message[]; active: boolean; legacyLive: boolean; fresh: boolean }) {
  // Sessions that don't report states: the last call of the last group runs while the agent works.
  const states = msgs.map((m, i) => (m.state ? toolState(m, active) : legacyLive && i === msgs.length - 1 ? "running" : "unknown"));
  const running = states.includes("running");
  const failed = states.includes("failed");
  const [open, setOpen] = useState(running);
  useEffect(() => setOpen(running), [running]);
  const names = [...new Set(msgs.map((m) => m.tool ?? "tool"))];
  return (
    <div className={`tools${fresh ? " fresh" : ""}`} data-open={open} data-running={running || undefined}>
      <button className="tools-head" onClick={() => setOpen(!open)} aria-expanded={open}>
        {running ? <span className="mini-spin" aria-hidden /> : <Icon name={failed ? "x" : "check"} size={15} className={failed ? "tone-fail" : undefined} />}
        <span className="names">
          {msgs.length} {msgs.length === 1 ? "step" : "steps"} · {names.slice(0, 3).join(", ")}
          {names.length > 3 ? "…" : ""}
        </span>
        <Icon name="chevron" size={16} className="chev" />
      </button>
      <div className="tools-body">
        <div>
          <ol className="tl">
            {msgs.map((m, i) => (
              <li key={m.id} className={states[i]}>
                <span className="tl-node">
                  <Icon name={toolIcon(m.tool)} />
                  {(states[i] === "done" || states[i] === "failed") && (
                    <span className="tl-badge">
                      <Icon name={states[i] === "done" ? "check" : "x"} size={8} />
                    </span>
                  )}
                </span>
                <span className="tl-name">{m.tool ?? "tool"}</span>
                <span className="tl-text">{toolText(m.tool, m.text)}</span>
                {m.background && <span className="tl-bg">{states[i] === "running" ? "background" : "bg"}</span>}
              </li>
            ))}
          </ol>
        </div>
      </div>
    </div>
  );
}

const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(s / 60);
  return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : `${m}:${String(s % 60).padStart(2, "0")}`;
};

/** In the chat, below the last message: what the agent is doing right now. */
function Activity({ pane, current, thinking }: { pane: Pane; current: Message | null; thinking: boolean }) {
  useTick(1000);
  return (
    <div className="msg assistant activity" role="status">
      <Avatar pane={pane} small />
      <div className="activity-card">
        <span className="wave" aria-hidden>
          <i />
          <i />
          <i />
          <i />
          <i />
        </span>
        <span className="activity-text">
          {current ? (
            <>
              <b>{current.tool ?? "Tool"}</b> {toolText(current.tool, current.text)}
            </>
          ) : thinking ? (
            "Thinking…"
          ) : (
            pane.message
          )}
        </span>
        <span className="activity-clock">{clock(Date.now() - (current?.ts ?? pane.updatedAt))}</span>
      </div>
    </div>
  );
}

/** Background shells and subagents still running, whatever the turn is doing. */
function Background({ items }: { items: Message[] }) {
  useTick(1000);
  return (
    <div className="bg-tray" role="status">
      <div className="bg-head">
        <span className="mini-spin" aria-hidden />
        <b>Background</b>
        <span>{items.length} running</span>
      </div>
      <ul>
        {items.map((m) => (
          <li key={m.id}>
            <span className="tl-node">
              <Icon name={toolIcon(m.tool)} />
            </span>
            <span className="bg-text">
              <b>{m.tool ?? "Task"}</b>
              <span>{toolText(m.tool, m.text)}</span>
            </span>
            {m.ts && <span className="activity-clock">{clock(Date.now() - m.ts)}</span>}
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

  // Refetch on every status change for this pane, every 4 s while it works, and every 6 s while
  // background work runs (it reports back without a status change).
  const statusKey = pane ? `${pane.status}:${pane.updatedAt}` : "";
  useEffect(() => {
    if (!ready) return;
    const load = () => client.request({ t: "thread.get", paneId, limit: 200 }).catch(() => {});
    load();
    const every = pane?.status === "working" ? 4000 : bgRunning ? 6000 : 0;
    if (!every) return;
    const t = setInterval(load, every);
    return () => clearInterval(t);
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
  }, [messages.length, pane?.request, pane?.status]);

  async function act(kind: "send" | "approve" | "decline" | "interrupt", text = draft.trim()) {
    if (busy) return;
    if (kind === "send" && !text) return;
    setBusy(kind);
    setErr(null);
    haptic(kind === "interrupt" ? [6, 30, 6] : 8);
    try {
      // Text for a "Type something" row answers through that row.
      const msg =
        kind === "send" && typing !== null ? ({ t: "choose", paneId, option: typing, text } as const) : kind === "send" ? ({ t: "send", paneId, text } as const) : ({ t: kind, paneId } as const);
      await client.request(msg);
      if (kind === "send") {
        setDraft("");
        setTyping(null);
        if (input.current) input.current.style.height = "";
      }
      stick.current = true;
    } catch (e) {
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

  return (
    <div className="screen">
      <header className="thread-head">
        <button className="icon-btn" aria-label="Back" onClick={() => go("/")}>
          <Icon name="back" />
        </button>
        {pane && <Avatar pane={pane} small morph />}
        <div className="thread-title">
          <b style={{ viewTransitionName: vtName("tt", paneId) } as CSSProperties}>{pane?.title ?? "Agent"}</b>
          <span>{pane ? [pane.workspace, info?.name].filter(Boolean).join(" · ") : state.panes.length ? "This agent's pane was closed" : "Loading…"}</span>
        </div>
      </header>

      <div
        className="scroll thread-scroll"
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
          if (stick.current) setUnseen(false);
        }}
      >
        <div className="messages">
          <ConnNotice state={state} onRetry={() => client.reconnect()} />
          {!thread && ready && (
            <div className="thinking" aria-label="Loading">
              <i />
              <i />
              <i />
            </div>
          )}
          {thread && messages.length === 0 && pane?.reply && (
            <div className="msg assistant">
              <Avatar pane={pane} small />
              <div className="bubble">
                <Markdown text={pane.reply} />
              </div>
            </div>
          )}
          {thread && messages.length === 0 && !pane?.reply && (
            <div className="empty">
              <div className="orbit-art">
                <i />
                <i />
                <img src="./icons/logo.svg" alt="" />
              </div>
              <h3>A fresh start</h3>
              <p className="sub">Send a message and it's typed into the agent on your desktop.</p>
            </div>
          )}
          {all.map((b, i) => {
            if (b.kind === "tools")
              return <Tools key={`t-${b.msgs[0].id}`} msgs={b.msgs} active={active} legacyLive={working && i === all.length - 1} fresh={isFresh(b.msgs[0].id)} />;
            const m = b.msg;
            const fresh = isFresh(m.id) ? " fresh" : "";
            if (m.role === "user")
              return (
                <div key={m.id} className={`msg user${fresh}`}>
                  <div className="bubble">
                    <p>{m.text}</p>
                    {m.ts && <span className="msg-time">{ago(m.ts)}</span>}
                  </div>
                </div>
              );
            const prev = all[i - 1];
            const head = !prev || prev.kind !== "msg" || prev.msg.role !== "assistant";
            return (
              <div key={m.id} className={`msg assistant${fresh}`}>
                {pane && head ? <Avatar pane={{ ...pane, status: "idle" }} small /> : <span style={{ width: 28, flex: "none" }} />}
                <div className="bubble">{screen ? <pre className="screen-text">{m.text}</pre> : <Markdown text={m.text} />}</div>
              </div>
            );
          })}
          {pane && background.length > 0 && <Background items={background} />}
          {pane && working && <Activity pane={pane} current={current} thinking={thinking} />}
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
            <Icon name="down" size={18} />
            <span className="dot" />
          </button>
        )}
        {req?.kind === "approval" && (
          <div className="sheet" key={`a-${req.text}`}>
            <div className="sheet-label">
              <Icon name="shield" size={14} /> Permission request
            </div>
            <pre className="request-code">{req.text}</pre>
            <div className="sheet-row">
              <SlideToConfirm label="Slide to approve" doneLabel="Approved" disabled={!ready} onConfirm={() => act("approve")} />
              <button className="btn" style={{ height: 54, borderRadius: 999 }} disabled={!ready || !!busy} onClick={() => void act("decline").catch(() => {})} aria-label="Decline">
                <Icon name="x" />
              </button>
            </div>
            {extra.length > 0 && (
              <div className="alt-options">
                {extra.map((o) => (
                  <button key={o.n} className="alt-option" data-picked={picked === o.n} disabled={!ready || picked !== null} onClick={() => void choose(o)}>
                    <span className="opt-n">{o.n}</span>
                    {o.label}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
        {req?.kind === "question" && (
          <div className="sheet question" key={`q-${req.text}`}>
            <div className="sheet-label">
              <Icon name="question" size={14} /> {info?.name ?? "The agent"} asks
            </div>
            <p className="request-q">{req.text}</p>
            {options.length > 0 && (
              <div className="options" data-busy={picked !== null}>
                {options.filter((o) => !isFreeText(o)).map((o, i) => (
                  <button
                    key={o.n}
                    className="option"
                    style={{ "--i": i } as CSSProperties}
                    data-picked={picked === o.n || typing === o.n}
                    data-free={isFreeText(o) || undefined}
                    disabled={!ready || (picked !== null && picked !== o.n)}
                    onClick={() => void choose(o)}
                  >
                    <span className="opt-n">{isFreeText(o) ? <Icon name="pencil" size={13} /> : o.n}</span>
                    <span className="opt-body">
                      <b>{o.label}</b>
                      {o.detail && <span>{o.detail}</span>}
                    </span>
                    <span className="opt-go">{picked === o.n ? <span className="opt-spin" /> : <Icon name={isFreeText(o) ? "pencil" : "right"} size={16} />}</span>
                  </button>
                ))}
                {options.filter(isFreeText).map((o) => (
                  <button key={o.n} className="option-free" data-picked={typing === o.n} disabled={!ready || picked !== null} onClick={() => void choose(o)}>
                    <Icon name="pencil" size={13} /> {typing === o.n ? "Type your answer below" : "Or type your own answer"}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
        {err && <div className="inline-error">{err}</div>}
        {quick && ready && (
          <div className="chips">
            {quick.map((q, n) => (
              <button
                key={q}
                className="chip"
                style={{ "--i": n } as CSSProperties}
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
        <div className="composer">
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
          <button
            className="send"
            data-mode={mode}
            aria-label={mode === "stop" ? "Interrupt" : "Send"}
            disabled={!ready || !!busy || mode === "idle"}
            onClick={() => void act(mode === "stop" ? "interrupt" : "send").catch(() => {})}
          >
            <Icon name="send" className="i-send" />
            <Icon name="stop" className="i-stop" />
          </button>
        </div>
      </footer>
    </div>
  );
}
