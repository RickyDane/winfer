// Shared pieces of the side-panel screens (Browser, Android, Terminal, Files).
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DesktopClient } from "../client";
import { go, panelBack } from "../hooks";
import type { DesktopMsg, PhoneMsg, StreamMsg } from "../protocol";
import { Icon, type IconName } from "./common";

/** A panel request's answer (`ack.value`); throws the desktop's error. */
export async function call<T>(client: DesktopClient, msg: PhoneMsg): Promise<T> {
  const reply = await client.request(msg);
  if (reply.t === "ack" && !reply.ok) throw new Error(reply.error || "Desktop refused");
  return (reply.t === "ack" ? reply.value : reply) as T;
}

/** Standard base64 (the desktop's encoding for terminal bytes and frames). */
export function fromB64(s: string): Uint8Array<ArrayBuffer> {
  const bin = atob(s);
  const out = new Uint8Array(new ArrayBuffer(bin.length));
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function toB64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export const textToB64 = (text: string) => toB64(new TextEncoder().encode(text));

/** Ctrl+key as the control character a terminal sends (Ctrl+C = \x03). */
export function withCtrl(data: string): string {
  const c = data[0];
  if (!c) return data;
  const code = c.toUpperCase().charCodeAt(0);
  if (code >= 64 && code <= 95) return String.fromCharCode(code - 64) + data.slice(1);
  if (c === " ") return "\x00" + data.slice(1);
  return data;
}

/** The workspace the panel shows, remembered across visits. */
const WS_KEY = "winfer.panelWorkspace";
export const savedWorkspace = () => localStorage.getItem(WS_KEY);
export const saveWorkspace = (id: string) => localStorage.setItem(WS_KEY, id);

/** Runs `fn` now and every `ms` while the page is visible. */
export function usePoll(fn: () => void, ms: number, deps: unknown[]) {
  const ref = useRef(fn);
  ref.current = fn;
  useEffect(() => {
    ref.current();
    const t = setInterval(() => document.visibilityState === "visible" && ref.current(), ms);
    const vis = () => document.visibilityState === "visible" && ref.current();
    document.addEventListener("visibilitychange", vis);
    return () => {
      clearInterval(t);
      document.removeEventListener("visibilitychange", vis);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

export interface Frame {
  url: string;
  /** The picture's coordinate space: CSS pixels of the page, or the device's screen. */
  width: number;
  height: number;
  seq: number;
}

/** Live frames of `feed` as object URLs; each is acknowledged once drawn (call `drawn`). */
export function useFrames(client: DesktopClient, feed: "browser" | "android") {
  const [frame, setFrame] = useState<Frame | null>(null);
  const [error, setError] = useState<string | null>(null);
  const urls = useRef<string[]>([]);
  useEffect(() => {
    const off = client.onMessage((msg: DesktopMsg) => {
      const m = msg as StreamMsg;
      if (m.t === "panel.error" && m.feed === feed) setError(m.error);
      if (m.t !== "panel.frame" || m.feed !== feed) return;
      const url = URL.createObjectURL(new Blob([fromB64(m.data)], { type: m.mime ?? "image/jpeg" }));
      urls.current.push(url);
      setError(null);
      setFrame({ url, width: m.width, height: m.height, seq: m.seq });
    });
    return () => {
      off();
      urls.current.forEach((u) => URL.revokeObjectURL(u));
      urls.current = [];
    };
  }, [client, feed]);
  /** The <img> showed frame `seq`: older pictures go, the desktop sends the next. */
  const drawn = (seq: number, url: string) => {
    const keep = urls.current.indexOf(url);
    urls.current.slice(0, Math.max(0, keep)).forEach((u) => URL.revokeObjectURL(u));
    urls.current = urls.current.slice(Math.max(0, keep));
    client.fire({ t: "panel.frameDone", seq });
  };
  return { frame, error, drawn };
}

/** A screen's bar: back to where it was opened from (the Panel tab or a chat), a title and its actions. */
export function PanelBar({ title, subtitle, back, children }: { title: string; subtitle?: string; back?: string | (() => void); children?: ReactNode }) {
  return (
    <header className="bar">
      <button className="icon-btn" aria-label="Back" onClick={() => (typeof back === "function" ? back() : go(back ?? panelBack()))}>
        <Icon name="back" size={19} />
      </button>
      <div className="bar-title">
        <b>{title}</b>
        {subtitle && <span>{subtitle}</span>}
      </div>
      {children}
    </header>
  );
}

export function BarButton({ icon, label, onClick, active, disabled }: { icon: IconName; label: string; onClick: () => void; active?: boolean; disabled?: boolean }) {
  return (
    <button className="icon-btn" aria-label={label} title={label} data-on={active || undefined} disabled={disabled} onClick={onClick}>
      <Icon name={icon} size={18} />
    </button>
  );
}

/** One line typed into the page or device: sent with ⏎ (or the send button), plus quick keys. */
export function ComposeBar({ placeholder, onText, keys }: { placeholder: string; onText: (text: string, enter: boolean) => void; keys: { label: ReactNode; title: string; onPress: () => void }[] }) {
  const [text, setText] = useState("");
  const send = (enter: boolean) => {
    if (!text && !enter) return;
    onText(text, enter);
    setText("");
  };
  return (
    <div className="compose-bar">
      <div className="compose-keys">
        {keys.map((k) => (
          <button key={k.title} className="key-btn" title={k.title} aria-label={k.title} onClick={k.onPress}>
            {k.label}
          </button>
        ))}
      </div>
      <form
        className="compose-line"
        onSubmit={(e) => {
          e.preventDefault();
          send(true);
        }}
      >
        <input value={text} onChange={(e) => setText(e.target.value)} placeholder={placeholder} autoCapitalize="off" autoCorrect="off" spellCheck={false} enterKeyHint="send" />
        <button type="button" className="key-btn" disabled={!text} onClick={() => send(false)} title="Type it without Enter">
          Type
        </button>
      </form>
    </div>
  );
}

/** A centered note with an optional action (loading, errors, nothing yet). */
export function Notice({ icon, title, text, action }: { icon: IconName; title: string; text?: ReactNode; action?: { label: string; onClick: () => void; busy?: boolean } }) {
  return (
    <div className="empty">
      <span className="empty-mark">
        <Icon name={icon} />
      </span>
      <h3>{title}</h3>
      {text && <p className="sub">{text}</p>}
      {action && (
        <button className="btn primary panel-notice-btn" disabled={action.busy} onClick={action.onClick}>
          {action.busy ? "Working…" : action.label}
        </button>
      )}
    </div>
  );
}
