// A desktop terminal on the phone: the workspace's side terminal (or a terminal pane) streamed
// over `term.*`, typed into with the phone's keyboard and a row of keys phones lack.
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef, useState } from "react";
import type { DesktopClient } from "../client";
import { haptic, useClient } from "../hooks";
import type { DesktopMsg, StreamMsg } from "../protocol";
import { linkState } from "./common";
import { BarButton, PanelBar, fromB64, textToB64, withCtrl } from "./panelKit";

const FONT_KEY = "winfer.termFont";

/** Keys a phone keyboard lacks, as the bytes a terminal sends for them. */
const KEYS: { label: string; title: string; data?: string; ctrl?: true }[] = [
  { label: "Esc", title: "Escape", data: "\x1b" },
  { label: "Tab", title: "Tab", data: "\t" },
  { label: "Ctrl", title: "Control (next key)", ctrl: true },
  { label: "^C", title: "Interrupt", data: "\x03" },
  { label: "↑", title: "Up", data: "\x1b[A" },
  { label: "↓", title: "Down", data: "\x1b[B" },
  { label: "←", title: "Left", data: "\x1b[D" },
  { label: "→", title: "Right", data: "\x1b[C" },
  { label: "|", title: "Pipe", data: "|" },
  { label: "~", title: "Tilde", data: "~" },
  { label: "/", title: "Slash", data: "/" },
  { label: "-", title: "Dash", data: "-" },
];

function cssVar(name: string, fallback: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

export function Term({ client, workspaceId, paneId }: { client: DesktopClient; workspaceId: string; paneId: string }) {
  const state = useClient(client);
  const online = linkState(state) === "online";
  const ws = state.workspaces.find((w) => w.id === workspaceId);
  const pane = state.panes.find((p) => p.id === paneId);
  const host = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fit = useRef<FitAddon | null>(null);
  const ctrl = useRef(false);
  const [ctrlOn, setCtrlOn] = useState(false);
  const [exited, setExited] = useState<string | null>(null);
  const [font, setFont] = useState(() => Number(localStorage.getItem(FONT_KEY)) || 12);
  const size = useRef({ cols: 0, rows: 0 });

  const send = (data: string) => client.fire({ t: "term.in", paneId, data: textToB64(data) });

  useEffect(() => {
    const t = new Terminal({
      fontSize: font,
      fontFamily: cssVar("--mono", "ui-monospace, Menlo, monospace"),
      cursorBlink: true,
      scrollback: 5000,
      allowProposedApi: true,
      convertEol: false,
      theme: { background: cssVar("--code-bg", "#09090b"), foreground: cssVar("--fg", "#ececf0"), cursor: cssVar("--accent", "#7c8cff"), selectionBackground: "rgba(124,140,255,0.35)" },
    });
    const f = new FitAddon();
    t.loadAddon(f);
    t.open(host.current!);
    term.current = t;
    fit.current = f;
    t.onData((data) => {
      if (ctrl.current) {
        ctrl.current = false;
        setCtrlOn(false);
        data = withCtrl(data);
      }
      send(data);
    });
    const off = client.onMessage((msg: DesktopMsg) => {
      const m = msg as StreamMsg;
      if (!("paneId" in m) || m.paneId !== paneId) return;
      if (m.t === "term.snapshot") {
        t.reset();
        t.write(fromB64(m.data));
        setExited(null);
      } else if (m.t === "term.out") t.write(fromB64(m.data));
      else if (m.t === "term.exit") setExited(m.gone ? "This terminal is gone on the desktop." : `The process exited (${m.code}).`);
    });
    return () => {
      off();
      t.dispose();
      term.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, paneId]);

  useEffect(() => {
    const t = term.current;
    if (!t) return;
    t.options.fontSize = font;
    localStorage.setItem(FONT_KEY, String(font));
    fit.current?.fit();
  }, [font]);

  // Attach (and again after reconnecting or coming back); the phone sizes the terminal while it shows it.
  useEffect(() => {
    if (!online) return;
    const attach = () => {
      if (document.visibilityState !== "visible" || !term.current) return;
      fit.current?.fit();
      const { cols, rows } = term.current;
      size.current = { cols, rows };
      client.fire({ t: "term.attach", paneId, cols, rows });
    };
    attach();
    const ro = new ResizeObserver(() => {
      const t = term.current;
      if (!t) return;
      fit.current?.fit();
      if (t.cols !== size.current.cols || t.rows !== size.current.rows) {
        size.current = { cols: t.cols, rows: t.rows };
        client.fire({ t: "term.resize", paneId, cols: t.cols, rows: t.rows });
      }
    });
    ro.observe(host.current!);
    document.addEventListener("visibilitychange", attach);
    return () => {
      ro.disconnect();
      document.removeEventListener("visibilitychange", attach);
      client.fire({ t: "term.detach", paneId });
    };
  }, [client, paneId, online]);

  const side = paneId.startsWith("side-terminal-");
  return (
    <div className="screen panel-screen term-screen">
      <PanelBar title={side ? "Terminal" : (pane?.title ?? "Terminal")} subtitle={ws?.name}>
        <BarButton icon="minus" label="Smaller text" onClick={() => setFont((f) => Math.max(8, f - 1))} />
        <BarButton icon="plus" label="Larger text" onClick={() => setFont((f) => Math.min(20, f + 1))} />
      </PanelBar>
      {exited && <p className="inline-error panel-error">{exited}</p>}
      {!online && <p className="inline-error panel-error">The desktop is offline.</p>}
      <div className="term-host" ref={host} onClick={() => term.current?.focus()} />
      <div className="term-keys">
        {KEYS.map((k) => (
          <button
            key={k.title}
            className="key-btn"
            title={k.title}
            aria-label={k.title}
            data-on={(k.ctrl && ctrlOn) || undefined}
            onPointerDown={(e) => e.preventDefault()}
            onClick={() => {
              haptic(4);
              if (k.ctrl) {
                ctrl.current = !ctrl.current;
                setCtrlOn(ctrl.current);
              } else if (k.data) send(k.data);
              term.current?.focus();
            }}
          >
            {k.label}
          </button>
        ))}
      </div>
    </div>
  );
}
