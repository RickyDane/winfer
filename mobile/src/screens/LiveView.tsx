// A live picture from the desktop (a browser tab or an Android screen) that takes the phone's
// fingers. "page" mode: tap = click, drag = scroll, pinch = zoom the picture here (then drag
// pans it). "touch" mode: every finger move goes to the device as is (Android swipes and drags).
import { useEffect, useRef, useState } from "react";
import type { Frame } from "./panelKit";

/** A point on the picture as a fraction of it (0..1 both ways). */
export type Unit = { x: number; y: number };

interface Props {
  frame: Frame | null;
  drawn: (seq: number, url: string) => void;
  mode: "page" | "touch";
  onTap?: (at: Unit) => void;
  /** page: a scroll by this many picture pixels at a point. */
  onScroll?: (at: Unit, dx: number, dy: number) => void;
  /** touch: a finger went down, moved or lifted. */
  onTouch?: (kind: "down" | "move" | "up", at: Unit) => void;
  placeholder?: React.ReactNode;
}

const MOVE = 8;

export function LiveView({ frame, drawn, mode, onTap, onScroll, onTouch, placeholder }: Props) {
  const img = useRef<HTMLImageElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const [zoom, setZoom] = useState({ z: 1, x: 0, y: 0 });
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const gesture = useRef<{ x: number; y: number; t: number; moved: boolean; pinch?: { d: number; z: number; cx: number; cy: number; ox: number; oy: number } } | null>(null);
  const pending = useRef({ dx: 0, dy: 0, at: null as Unit | null, raf: 0 });
  const lastMove = useRef(0);

  // A new tab or device starts unzoomed.
  const shape = frame ? `${frame.width}x${frame.height}` : "";
  useEffect(() => setZoom({ z: 1, x: 0, y: 0 }), [shape]);

  const unit = (cx: number, cy: number): Unit | null => {
    const r = img.current?.getBoundingClientRect();
    if (!r || !r.width || !r.height) return null;
    return { x: Math.min(1, Math.max(0, (cx - r.left) / r.width)), y: Math.min(1, Math.max(0, (cy - r.top) / r.height)) };
  };

  const flushScroll = () => {
    const p = pending.current;
    p.raf = 0;
    if (p.at && (p.dx || p.dy)) onScroll?.(p.at, p.dx, p.dy);
    p.dx = 0;
    p.dy = 0;
  };

  const down = (e: React.PointerEvent) => {
    (e.target as Element).setPointerCapture?.(e.pointerId);
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (mode === "touch") {
      if (pointers.current.size === 1) {
        const at = unit(e.clientX, e.clientY);
        if (at) onTouch?.("down", at);
      }
      return;
    }
    if (pointers.current.size === 2) {
      const [a, b] = [...pointers.current.values()];
      const z = zoomRef.current;
      gesture.current = { x: 0, y: 0, t: 0, moved: true, pinch: { d: Math.hypot(a.x - b.x, a.y - b.y), z: z.z, cx: (a.x + b.x) / 2, cy: (a.y + b.y) / 2, ox: z.x, oy: z.y } };
    } else {
      gesture.current = { x: e.clientX, y: e.clientY, t: Date.now(), moved: false };
    }
  };

  const move = (e: React.PointerEvent) => {
    const prev = pointers.current.get(e.pointerId);
    if (!prev) return;
    const now = { x: e.clientX, y: e.clientY };
    pointers.current.set(e.pointerId, now);
    if (mode === "touch") {
      // At most one move a frame's worth (~60/s).
      if (pointers.current.size === 1 && performance.now() - lastMove.current > 16) {
        lastMove.current = performance.now();
        const at = unit(e.clientX, e.clientY);
        if (at) onTouch?.("move", at);
      }
      return;
    }
    const g = gesture.current;
    if (!g) return;
    if (g.pinch && pointers.current.size >= 2) {
      const [a, b] = [...pointers.current.values()];
      const z = Math.min(5, Math.max(1, (g.pinch.z * Math.hypot(a.x - b.x, a.y - b.y)) / Math.max(1, g.pinch.d)));
      const cx = (a.x + b.x) / 2;
      const cy = (a.y + b.y) / 2;
      setZoom(clamp({ z, x: g.pinch.ox + (cx - g.pinch.cx), y: g.pinch.oy + (cy - g.pinch.cy) }));
      return;
    }
    if (!g.moved && Math.hypot(now.x - g.x, now.y - g.y) < MOVE) return;
    g.moved = true;
    const dx = now.x - prev.x;
    const dy = now.y - prev.y;
    const z = zoomRef.current;
    if (z.z > 1.01) {
      setZoom(clamp({ ...z, x: z.x + dx, y: z.y + dy }));
      return;
    }
    // Scrolling the page: finger up = content up = wheel down; in the picture's pixels.
    const r = img.current?.getBoundingClientRect();
    const scale = r && frame ? frame.width / r.width : 1;
    const p = pending.current;
    p.at ??= unit(g.x, g.y);
    p.dx -= dx * scale;
    p.dy -= dy * scale;
    if (!p.raf) p.raf = requestAnimationFrame(flushScroll);
  };

  const up = (e: React.PointerEvent) => {
    const had = pointers.current.delete(e.pointerId);
    if (!had) return;
    if (mode === "touch") {
      if (pointers.current.size === 0) {
        const at = unit(e.clientX, e.clientY);
        if (at) onTouch?.("up", at);
      }
      return;
    }
    const g = gesture.current;
    if (pointers.current.size > 0) return;
    gesture.current = null;
    pending.current.at = null;
    if (!g || g.moved || g.pinch) return;
    const at = unit(e.clientX, e.clientY);
    if (at) onTap?.(at);
  };

  /** Keeps the zoomed picture over the box. */
  const clamp = (z: { z: number; x: number; y: number }) => {
    const r = box.current?.getBoundingClientRect();
    if (!r || z.z <= 1) return { z: 1, x: 0, y: 0 };
    const maxX = ((z.z - 1) * r.width) / 2;
    const maxY = ((z.z - 1) * r.height) / 2;
    return { z: z.z, x: Math.min(maxX, Math.max(-maxX, z.x)), y: Math.min(maxY, Math.max(-maxY, z.y)) };
  };

  return (
    <div
      ref={box}
      className="live-view"
      data-mode={mode}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={up}
      onDoubleClick={() => setZoom({ z: 1, x: 0, y: 0 })}
      onContextMenu={(e) => e.preventDefault()}
    >
      {frame ? (
        <img
          ref={img}
          src={frame.url}
          alt=""
          draggable={false}
          style={{ aspectRatio: `${frame.width} / ${frame.height}`, transform: zoom.z > 1 ? `translate(${zoom.x}px, ${zoom.y}px) scale(${zoom.z})` : undefined }}
          onLoad={() => drawn(frame.seq, frame.url)}
          onError={() => drawn(frame.seq, frame.url)}
        />
      ) : (
        placeholder
      )}
    </div>
  );
}
