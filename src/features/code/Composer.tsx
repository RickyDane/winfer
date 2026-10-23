import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowUp, ShieldCheck, Square } from "lucide-react";
import clsx from "clsx";
import { useContext } from "react";
import { CodeStoreContext, useCode } from "./store";
import { runCommand } from "./commands";
import { useRate } from "./Sheets";
import { useCodeWeb } from "./styles";
import { COMMANDS, MODES } from "./protocol";
import { DiffView } from "./Transcript";

function Approvals() {
  const approvals = useCode((s) => s.approvals);
  const send = useCode((s) => s.send);
  const a = approvals[0];
  if (!a) return null;
  return (
    <div className="cw-approval" key={a.id}>
      <div className="cw-approval-head">
        <ShieldCheck size={16} />
        <div>
          <b>{a.title}</b>
          <small>{a.detail}</small>
        </div>
        {approvals.length > 1 && <span className="cw-chip">+{approvals.length - 1} more</span>}
      </div>
      {a.diff && (
        <div className="cw-approval-diff">
          <DiffView diff={a.diff} max={14} />
        </div>
      )}
      <div className="cw-approval-actions">
        <button className="cw-btn primary" onClick={() => send({ type: "approve", id: a.id, decision: "allow" })}>
          Allow <kbd>Y</kbd>
        </button>
        <button className="cw-btn" onClick={() => send({ type: "approve", id: a.id, decision: "always" })}>
          Always allow {a.tool} <kbd>A</kbd>
        </button>
        <button className="cw-btn ghost danger" onClick={() => send({ type: "approve", id: a.id, decision: "deny" })}>
          Deny <kbd>N</kbd>
        </button>
      </div>
    </div>
  );
}

function fmtTokens(n: number) {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
}

export function Composer({ focused }: { focused: boolean }) {
  const { busy, mode, send, submit, usage, turnStarted, approvals, sheet } = useCode();
  const store = useContext(CodeStoreContext)!;
  const rate = useRate();
  const showRate = useCodeWeb((s) => s.showRate);
  const [text, setText] = useState("");
  const [sel, setSel] = useState(0);
  const ta = useRef<HTMLTextAreaElement>(null);
  const [, tick] = useState(0);
  const startOut = useRef(0);

  useEffect(() => {
    if (!busy) return;
    startOut.current = usage.output;
    const t = setInterval(() => tick((n) => n + 1), 250);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy]);

  const cmds = useMemo(
    () => (text.startsWith("/") && !text.includes(" ") ? COMMANDS.filter(([c]) => c.startsWith(text)) : []),
    [text],
  );

  // Approval keys work while the composer is empty.
  useEffect(() => {
    if (!focused || sheet) return;
    const on = (e: KeyboardEvent) => {
      const a = approvals[0];
      if (e.key === "Escape" && busy) {
        send({ type: "interrupt" });
        return;
      }
      if (e.key === "Tab" && e.shiftKey) {
        e.preventDefault();
        const i = MODES.findIndex((m) => m.id === mode);
        send({ type: "set_mode", mode: MODES[(i + 1) % MODES.length].id });
        return;
      }
      if (!a || text || e.metaKey || e.ctrlKey) return;
      const d = { y: "allow", a: "always", n: "deny" }[e.key.toLowerCase()] as "allow" | "always" | "deny" | undefined;
      if (d) {
        e.preventDefault();
        send({ type: "approve", id: a.id, decision: d });
      }
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [focused, sheet, approvals, text, busy, mode, send]);

  useEffect(() => {
    const el = ta.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = Math.min(el.scrollHeight, 220) + "px";
  }, [text]);

  const go = () => {
    if (!runCommand(text, store)) submit(text);
    setText("");
  };

  const elapsed = turnStarted ? (Date.now() - turnStarted) / 1000 : 0;

  return (
    <>
      <Approvals />
      <div className={clsx("cw-status", busy && "on")}>
        <span className="cw-orb" />
        <span className="cw-shimmer">Working</span>
        <span className="sep">·</span>
        <span>{elapsed.toFixed(0)}s</span>
        <span className="sep">·</span>
        <span>↓ {fmtTokens(usage.turn ?? Math.max(0, usage.output - startOut.current))} tokens</span>
        {showRate && rate != null && (
          <>
            <span className="sep">·</span>
            <span>{rate.toFixed(0)} tok/s</span>
          </>
        )}
        <span className="grow" />
        <span className="hint">
          <kbd>esc</kbd> to interrupt
        </span>
      </div>
      <div className={clsx("cw-composer", busy && "busy")}>
        {cmds.length > 0 && (
          <div className="cw-slash">
            {cmds.map(([c, d], i) => (
              <button
                key={c}
                className={clsx("cw-slash-item", i === sel && "sel")}
                onMouseEnter={() => setSel(i)}
                onClick={() => {
                  setText(c + " ");
                  ta.current?.focus();
                }}
              >
                <b>{c}</b>
                <span>{d}</span>
              </button>
            ))}
          </div>
        )}
        <textarea
          ref={ta}
          value={text}
          rows={1}
          placeholder={busy ? "Steer the agent — it reads this at its next step…" : "Ask winfer code…"}
          onChange={(e) => {
            setText(e.target.value);
            setSel(0);
          }}
          onKeyDown={(e) => {
            if (cmds.length) {
              if (e.key === "ArrowDown") return (e.preventDefault(), setSel((sel + 1) % cmds.length));
              if (e.key === "ArrowUp") return (e.preventDefault(), setSel((sel - 1 + cmds.length) % cmds.length));
              if (e.key === "Tab") return (e.preventDefault(), setText(cmds[sel][0] + " "));
            }
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              if (cmds.length && text !== cmds[sel][0]) setText(cmds[sel][0] + " ");
              else go();
            }
          }}
        />
        <div className="cw-composer-bar">
          <span className="cw-hint">
            <kbd>/</kbd> commands <kbd>⇧⇥</kbd> mode <kbd>esc</kbd> interrupt
          </span>
          <span className="grow" />
          {busy && !text ? (
            <button className="cw-send stop" onClick={() => send({ type: "interrupt" })} title="Interrupt (esc)">
              <Square size={13} fill="currentColor" />
            </button>
          ) : (
            <button className="cw-send" disabled={!text.trim()} onClick={go} title="Send (enter)">
              <ArrowUp size={16} />
            </button>
          )}
        </div>
      </div>
    </>
  );
}
