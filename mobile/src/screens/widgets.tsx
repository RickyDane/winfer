import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode, type RefObject } from "react";
import { haptic } from "../hooks";
import type { AgentStatus } from "../protocol";
import { Icon, type IconName } from "./common";

const reduced = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

/* ---------- Status ring: one arc per status, sized by count ---------- */
const RING_ORDER: AgentStatus[] = ["needs-input", "working", "error", "done", "idle"];
export function StatusRing({ counts, total, label }: { counts: Record<AgentStatus, number>; total: number; label: string }) {
  const r = 36;
  const c = 2 * Math.PI * r;
  const gap = total > 1 ? 7 : 0;
  let offset = 0;
  const hot = counts["needs-input"] > 0;
  return (
    <div className="ring" data-hot={hot} role="img" aria-label={`${total} agents`}>
      <svg viewBox="0 0 84 84">
        <circle className="track" cx="42" cy="42" r={r} />
        {RING_ORDER.map((s) => {
          const n = counts[s];
          const len = total ? (n / total) * c : 0;
          const seg = Math.max(0, len - gap);
          const el = (
            <circle key={s} className={`seg-${s}`} cx="42" cy="42" r={r} strokeDasharray={`${seg} ${c}`} strokeDashoffset={-offset} style={{ opacity: n ? 1 : 0 }} />
          );
          offset += len;
          return el;
        })}
      </svg>
      <div className="ring-center">
        <b>{hot ? counts["needs-input"] : total}</b>
        <small>{label}</small>
      </div>
    </div>
  );
}

/* ---------- Segmented control with a sliding thumb ---------- */
export function Segmented<T extends string>({ options, value, onChange }: { options: { value: T; label: string; count?: number; hot?: boolean }[]; value: T; onChange: (v: T) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [thumb, setThumb] = useState<CSSProperties>({ opacity: 0 });
  const counts = options.map((o) => o.count ?? 0).join();
  useLayoutEffect(() => {
    const root = ref.current;
    if (!root) return;
    const place = () => {
      const el = root.querySelector<HTMLElement>(`[data-v="${value}"]`);
      if (el) setThumb({ left: el.offsetLeft, width: el.offsetWidth });
    };
    place();
    // Counts appearing change button widths; so do fonts loading and rotation.
    const ro = new ResizeObserver(place);
    root.querySelectorAll("button").forEach((b) => ro.observe(b));
    return () => ro.disconnect();
  }, [value, options.length, counts]);
  return (
    <div className="seg" ref={ref} role="group">
      <span className="seg-thumb" style={thumb} />
      {options.map((o) => (
        <button
          key={o.value}
          data-v={o.value}
          aria-pressed={o.value === value}
          onClick={() => {
            haptic(5);
            onChange(o.value);
          }}
        >
          {o.label}
          {o.count !== undefined && o.count > 0 && <span className={`count${o.hot ? " hot" : ""}`}>{o.count}</span>}
        </button>
      ))}
    </div>
  );
}

/* ---------- Swipeable card: right = primary action, left = secondary ---------- */
type SwipeAction = { label: string; icon: IconName; run: () => void };
const ARM = 96;
export function SwipeCard({ right, left, onTap, onLongPress, children, status, index }: { right?: SwipeAction; left?: SwipeAction; onTap: () => void; onLongPress?: () => void; children: ReactNode; status: AgentStatus; index: number }) {
  const card = useRef<HTMLDivElement>(null);
  // Holding still on the card for a moment opens its menu instead of the thread.
  const hold = useRef<{ timer: number; fired: boolean } | null>(null);
  const release = () => {
    if (hold.current) clearTimeout(hold.current.timer);
  };
  const under = useRef<{ r: HTMLDivElement | null; l: HTMLDivElement | null }>({ r: null, l: null });
  const g = useRef<{ x: number; y: number; dx: number; axis: "x" | "y" | null; id: number; armed: boolean } | null>(null);
  const moved = useRef(false);

  const paint = (dx: number) => {
    const el = card.current;
    if (!el) return;
    el.style.transform = dx ? `translateX(${dx}px)` : "";
    const p = Math.min(1, Math.abs(dx) / ARM);
    if (under.current.r) {
      under.current.r.style.opacity = String(dx > 0 ? p : 0);
      under.current.r.style.setProperty("--armed-scale", dx >= ARM ? "1.15" : String(0.7 + p * 0.3));
    }
    if (under.current.l) {
      under.current.l.style.opacity = String(dx < 0 ? p : 0);
      under.current.l.style.setProperty("--armed-scale", -dx >= ARM ? "1.15" : String(0.7 + p * 0.3));
    }
  };

  return (
    <div className="swipe" style={{ "--i": Math.min(index, 8) } as CSSProperties} data-flip>
      {right && (
        <div className="swipe-under right" ref={(el) => void (under.current.r = el)}>
          <span className="swipe-icon">
            <Icon name={right.icon} size={16} />
          </span>
          {right.label}
        </div>
      )}
      {left && (
        <div className="swipe-under left" ref={(el) => void (under.current.l = el)}>
          {left.label}
          <span className="swipe-icon">
            <Icon name={left.icon} size={16} />
          </span>
        </div>
      )}
      <div
        ref={card}
        className="card"
        role="button"
        tabIndex={0}
        data-status={status}
        onKeyDown={(e) => e.key === "Enter" && onTap()}
        onContextMenu={(e) => {
          if (!onLongPress) return;
          e.preventDefault();
          // Right-click, or a touch browser's own long press: whichever comes first opens the menu once.
          if (hold.current?.fired) return;
          release();
          if (hold.current) hold.current.fired = true;
          haptic(14);
          onLongPress();
        }}
        onPointerDown={(e) => {
          if (e.pointerType === "mouse" && e.button !== 0) return;
          // Inline buttons handle their own taps.
          if ((e.target as HTMLElement).closest("button")) {
            g.current = null;
            return;
          }
          g.current = { x: e.clientX, y: e.clientY, dx: 0, axis: null, id: e.pointerId, armed: false };
          moved.current = false;
          release();
          hold.current = onLongPress
            ? {
                fired: false,
                timer: window.setTimeout(() => {
                  if (!hold.current || g.current?.axis) return;
                  hold.current.fired = true;
                  haptic(14);
                  onLongPress();
                }, 480),
              }
            : null;
        }}
        onPointerMove={(e) => {
          const s = g.current;
          if (!s || s.id !== e.pointerId) return;
          const dx = e.clientX - s.x;
          const dy = e.clientY - s.y;
          if (!s.axis && Math.hypot(dx, dy) > 8) {
            release();
            s.axis = Math.abs(dx) > Math.abs(dy) * 1.2 ? "x" : "y";
            if (s.axis === "x") {
              e.currentTarget.setPointerCapture(e.pointerId);
              e.currentTarget.dataset.dragging = "true";
            }
          }
          if (s.axis !== "x") return;
          moved.current = true;
          // Resist past the arm point; no travel toward a side without an action.
          let x = dx;
          if ((x > 0 && !right) || (x < 0 && !left)) x = x * 0.15;
          else if (Math.abs(x) > ARM) x = Math.sign(x) * (ARM + (Math.abs(x) - ARM) * 0.35);
          const armed = Math.abs(x) >= ARM && ((x > 0 && !!right) || (x < 0 && !!left));
          if (armed !== s.armed) haptic(armed ? 12 : 4);
          s.armed = armed;
          s.dx = x;
          paint(x);
        }}
        onPointerUp={(e) => {
          const s = g.current;
          g.current = null;
          const el = e.currentTarget;
          delete el.dataset.dragging;
          release();
          if (hold.current?.fired) return;
          if (!s || s.axis !== "x") {
            if (s && !moved.current) onTap();
            return;
          }
          if (s.armed) {
            const act = s.dx > 0 ? right : left;
            // Fling off, then let the list re-render with the new status.
            el.style.transition = "transform .28s var(--ease)";
            paint(Math.sign(s.dx) * (el.offsetWidth + 40));
            setTimeout(() => {
              el.style.transition = "";
              paint(0);
            }, 380);
            act?.run();
          } else {
            paint(0);
          }
        }}
        onPointerCancel={(e) => {
          release();
          g.current = null;
          delete e.currentTarget.dataset.dragging;
          paint(0);
        }}
      >
        {children}
      </div>
    </div>
  );
}

/* ---------- Slide to confirm ---------- */
export function SlideToConfirm({ label, doneLabel, disabled, onConfirm }: { label: string; doneLabel: string; disabled?: boolean; onConfirm: () => Promise<void> | void }) {
  const ref = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x0: number; max: number; p: number } | null>(null);
  const [state, setState] = useState<"idle" | "drag" | "done">("idle");
  const set = (p: number) => ref.current?.style.setProperty("--p", String(p));

  return (
    <div
      ref={ref}
      className="slider"
      data-snap={state !== "drag"}
      data-done={state === "done"}
      aria-disabled={disabled}
      onPointerDown={(e) => {
        if (disabled || state === "done") return;
        const thumb = (e.target as HTMLElement).closest(".slider-thumb");
        if (!thumb) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { x0: e.clientX, max: e.currentTarget.offsetWidth - 54, p: 0 };
        setState("drag");
        haptic(6);
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        const p = Math.max(0, Math.min(1, (e.clientX - d.x0) / d.max));
        if (p > 0.92 && d.p <= 0.92) haptic(14);
        d.p = p;
        set(p);
      }}
      onPointerUp={async () => {
        const d = drag.current;
        drag.current = null;
        if (!d) return;
        if (d.p > 0.92) {
          set(1);
          setState("done");
          haptic([10, 40, 18]);
          try {
            await onConfirm();
          } catch {
            set(0);
            setState("idle");
          }
        } else {
          set(0);
          setState("idle");
        }
      }}
      onPointerCancel={() => {
        drag.current = null;
        set(0);
        setState("idle");
      }}
    >
      <div className="slider-fill" />
      <div className="slider-label">{state === "done" ? doneLabel : label}</div>
      <div className="slider-thumb" role="button" aria-label={label} tabIndex={0} onKeyDown={(e) => e.key === "Enter" && !disabled && (set(1), setState("done"), void onConfirm())}>
        <Icon name={state === "done" ? "check" : "right"} size={20} />
      </div>
    </div>
  );
}

/* ---------- Pull to refresh (touch only; iOS standalone has none) ---------- */
export function usePullToRefresh(scroller: RefObject<HTMLElement | null>, indicator: RefObject<HTMLElement | null>, refresh: () => Promise<unknown>) {
  useEffect(() => {
    const el = scroller.current;
    const ind = indicator.current;
    if (!el || !ind) return;
    let y0: number | null = null;
    let pull = 0;
    let busy = false;
    const show = (d: number) => {
      ind.style.opacity = String(Math.min(1, d / 50));
      ind.style.transform = `translateY(${Math.min(d, 90) - 10}px) scale(${0.6 + Math.min(1, d / 70) * 0.4})`;
      (ind.firstElementChild as HTMLElement | null)?.style.setProperty("transform", `rotate(${d * 4}deg)`);
    };
    const start = (e: TouchEvent) => {
      if (busy || el.scrollTop > 0) return;
      y0 = e.touches[0].clientY;
      pull = 0;
    };
    const move = (e: TouchEvent) => {
      if (y0 === null) return;
      const d = e.touches[0].clientY - y0;
      if (d <= 0) return;
      e.preventDefault();
      const next = d * 0.45;
      if (next >= 70 && pull < 70) haptic(10);
      pull = next;
      show(pull);
    };
    const end = async () => {
      if (y0 === null) return;
      y0 = null;
      if (pull < 70) {
        ind.style.transition = "transform .3s var(--ease), opacity .3s";
        show(0);
        setTimeout(() => (ind.style.transition = ""), 300);
        return;
      }
      busy = true;
      ind.dataset.busy = "true";
      ind.style.transition = "transform .3s var(--spring)";
      ind.style.transform = "translateY(46px) scale(1)";
      try {
        await Promise.all([refresh(), new Promise((r) => setTimeout(r, 600))]);
      } catch {
        /* the connection notice explains */
      }
      ind.dataset.busy = "false";
      show(0);
      setTimeout(() => (ind.style.transition = ""), 300);
      busy = false;
    };
    el.addEventListener("touchstart", start, { passive: true });
    el.addEventListener("touchmove", move, { passive: false });
    el.addEventListener("touchend", end);
    el.addEventListener("touchcancel", end);
    return () => {
      el.removeEventListener("touchstart", start);
      el.removeEventListener("touchmove", move);
      el.removeEventListener("touchend", end);
      el.removeEventListener("touchcancel", end);
    };
  }, [scroller, indicator, refresh]);
}

/* ---------- FLIP: animate children with [data-flip] to their new spot after a re-order ---------- */
export function useFlip(container: RefObject<HTMLElement | null>, key: string) {
  const prev = useRef(new Map<Element, DOMRect>());
  useLayoutEffect(() => {
    const el = container.current;
    if (!el) return;
    const items = Array.from(el.querySelectorAll("[data-flip]"));
    const next = new Map<Element, DOMRect>();
    for (const item of items) {
      const r = item.getBoundingClientRect();
      next.set(item, r);
      const old = prev.current.get(item);
      if (!old || reduced()) continue;
      const dy = old.top - r.top;
      if (Math.abs(dy) < 2) continue;
      item.animate([{ transform: `translateY(${dy}px)` }, { transform: "none" }], { duration: 520, easing: "cubic-bezier(.22,1,.36,1)" });
    }
    prev.current = next;
  }, [container, key]);
}

/** Collapse progress (0 → 1) of a large title as `scroller` scrolls `distance` px. */
export function useCollapse(scroller: RefObject<HTMLElement | null>, target: RefObject<HTMLElement | null>, distance = 60) {
  const onScroll = useCallback(() => {
    const s = scroller.current;
    const t = target.current;
    if (!s || !t) return;
    t.style.setProperty("--collapse", String(Math.min(1, Math.max(0, s.scrollTop / distance))));
  }, [scroller, target, distance]);
  useLayoutEffect(onScroll, [onScroll]);
  return onScroll;
}
