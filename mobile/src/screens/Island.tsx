import { useEffect, useRef, useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { go, haptic, useClient } from "../hooks";
import type { Pane } from "../protocol";
import { ProviderMark, STATUS_LABEL, providerInfo } from "./common";

const LOUD = new Set(["needs-input", "done", "error"]);

/** Dynamic-Island-style toast when another agent finishes or needs you while the app is open. */
export function Island({ client, openPaneId }: { client: DesktopClient; openPaneId: string | null }) {
  const state = useClient(client);
  const prev = useRef<Map<string, string> | null>(null);
  const [shown, setShown] = useState<Pane | null>(null);
  const [leaving, setLeaving] = useState(false);
  const drag = useRef<{ y: number; dy: number } | null>(null);
  const swiped = useRef(false);

  useEffect(() => {
    const now = new Map(state.panes.map((p) => [p.id, p.status]));
    const before = prev.current;
    prev.current = now;
    // Nothing to compare against until the first pane list arrives.
    if (!before || before.size === 0) return;
    const hit = state.panes.find((p) => LOUD.has(p.status) && before.get(p.id) !== p.status && p.id !== openPaneId && before.has(p.id));
    if (!hit) return;
    haptic(hit.status === "needs-input" ? [12, 60, 12] : 10);
    setLeaving(false);
    swiped.current = false;
    setShown(hit);
  }, [state.panes, openPaneId]);

  useEffect(() => {
    if (!shown) return;
    const t1 = setTimeout(() => setLeaving(true), 4200);
    const t2 = setTimeout(() => setShown(null), 4600);
    return () => {
      clearTimeout(t1);
      clearTimeout(t2);
    };
  }, [shown]);

  if (!shown) return null;
  const text = shown.request?.text.split("\n")[0] ?? shown.message ?? STATUS_LABEL[shown.status];
  const info = providerInfo(shown.provider);
  const dismiss = () => {
    setLeaving(true);
    setTimeout(() => setShown(null), 300);
  };
  return (
    <button
      className="island"
      data-status={shown.status}
      data-leaving={leaving}
      key={`${shown.id}:${shown.updatedAt}`}
      onPointerDown={(e) => (drag.current = { y: e.clientY, dy: 0 })}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        d.dy = Math.min(0, e.clientY - d.y);
        e.currentTarget.style.transform = `translateY(${d.dy}px)`;
      }}
      onPointerUp={(e) => {
        const d = drag.current;
        drag.current = null;
        e.currentTarget.style.transform = "";
        if (d && d.dy < -24) {
          swiped.current = true;
          dismiss();
        }
      }}
      onClick={() => {
        if (leaving || swiped.current) return;
        setShown(null);
        go(`/pane/${encodeURIComponent(shown.id)}`);
      }}
    >
      <span className="island-av" style={{ "--c": info?.tile ?? "#3a3a44", color: info?.fg ?? "#fff" } as CSSProperties}>
        <ProviderMark provider={shown.provider} size={19} />
        <span className="island-dot" />
      </span>
      <span className="island-text">
        <b>{shown.title}</b>
        <span>{text}</span>
      </span>
      <span className="island-chip">{shown.status === "needs-input" ? "Open" : STATUS_LABEL[shown.status]}</span>
      <span className="island-timer" />
    </button>
  );
}
