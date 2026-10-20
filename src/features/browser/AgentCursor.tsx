import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import type { PageSize } from "./browser";

/** An agent's mouse event in a tab, in page CSS pixels (the backend's `browser://agent-pointer`). */
interface PointerEvent {
  targetId: string;
  x: number;
  y: number;
  kind: "mouseMoved" | "mousePressed" | "mouseReleased" | "mouseWheel";
}

/** How long the cursor stays after the agent's last mouse event. */
const IDLE_MS = 2500;
/** How long a click's ripple lives (its delay plus animation; see `.agent-click`). */
const CLICK_MS = 900;

/**
 * The agent's mouse in the tab on screen: a cursor that glides to where the agent points and
 * pulses where it clicks. Drawn over the page, never taking input.
 */
export function AgentCursor({ tabId, page }: { tabId: string; page: PageSize }) {
  const [cursor, setCursor] = useState<{ x: number; y: number; shown: boolean; glide: boolean } | null>(null);
  const [presses, setPresses] = useState(0);
  const [clicks, setClicks] = useState<{ id: number; x: number; y: number }[]>([]);
  const nextClick = useRef(0);

  useEffect(() => {
    let idle = 0;
    const timers = new Set<number>();
    const unlisten = listen<PointerEvent>("browser://agent-pointer", ({ payload: p }) => {
      if (p.targetId !== tabId) return;
      // Glide from where it was; appearing, it starts at the point instead of sliding in from a corner.
      setCursor((c) => ({ x: p.x, y: p.y, shown: true, glide: !!c?.shown }));
      if (p.kind === "mousePressed") {
        const id = ++nextClick.current;
        setPresses((n) => n + 1);
        setClicks((cs) => [...cs, { id, x: p.x, y: p.y }]);
        const t = window.setTimeout(() => {
          timers.delete(t);
          setClicks((cs) => cs.filter((c) => c.id !== id));
        }, CLICK_MS);
        timers.add(t);
      }
      clearTimeout(idle);
      idle = window.setTimeout(() => setCursor((c) => c && { ...c, shown: false }), IDLE_MS);
    });
    return () => {
      clearTimeout(idle);
      timers.forEach(clearTimeout);
      void unlisten.then((off) => off());
    };
  }, [tabId]);

  if (!cursor || page.width < 1 || page.height < 1) return null;
  const at = (x: number, y: number) => ({ left: `${(x / page.width) * 100}%`, top: `${(y / page.height) * 100}%` });
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden">
      {clicks.map((c) => (
        <span key={c.id} className="agent-click" style={at(c.x, c.y)} />
      ))}
      <div className={cn("agent-cursor", cursor.glide && "agent-glide")} style={{ ...at(cursor.x, cursor.y), opacity: cursor.shown ? 1 : 0 }}>
        {/* Remounted per press, which restarts its press animation. */}
        <svg key={presses} width="22" height="22" viewBox="0 0 24 24" className={cn(presses > 0 && "agent-cursor-press")}>
          <path d="M3 2 L3 19.5 L7.8 15 L11.2 22 L14.2 20.6 L10.8 13.8 L17.5 13.8 Z" fill="var(--color-accent)" stroke="white" strokeWidth="1.6" strokeLinejoin="round" />
        </svg>
      </div>
    </div>
  );
}
