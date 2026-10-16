import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { setPageSize, type Device, type PageSize } from "./browser";

const MOBILE = { width: 390, height: 844, deviceScaleFactor: 2, mobile: true };

interface Viewport {
  width: number;
  height: number;
  deviceScaleFactor: number;
  mobile: boolean;
}

// CDP modifier bits.
const modifiers = (e: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }) =>
  (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);

const BUTTONS = ["left", "middle", "right"] as const;

const KEY_CODES: Record<string, number> = {
  Backspace: 8, Tab: 9, Enter: 13, Escape: 27, " ": 32, PageUp: 33, PageDown: 34, End: 35, Home: 36,
  ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Delete: 46,
};

const keyCode = (e: KeyboardEvent) => KEY_CODES[e.key] ?? (e.key.length === 1 ? e.key.toUpperCase().charCodeAt(0) : 0);

/** Smallest and largest page size the edges can be dragged to. */
const MIN_SIZE = 200;
const MAX_SIZE = 4096;
/** Room around a sized page for its drag handles. */
const GUTTER = 16;

const clamp = (n: number) => Math.round(Math.min(Math.max(n, MIN_SIZE), MAX_SIZE));

/**
 * Shows one tab via CDP screencast and forwards mouse, wheel and keyboard input.
 * The page fits the panel, or has a size of its own (dragged, set by an agent, or a phone),
 * scaled down to fit when the panel is smaller.
 */
export function BrowserView({ workspaceId, tabId, device, size }: { workspaceId: string; tabId: string; device: Device; size?: PageSize }) {
  const box = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const viewport = useRef<Viewport | null>(null);
  const attached = useRef(false);
  const [panel, setPanel] = useState({ width: 0, height: 0 });

  const own = size ?? (device === "mobile" ? MOBILE : null);
  const page = own ?? panel;
  const room = own ? { width: panel.width - 2 * GUTTER, height: panel.height - 2 * GUTTER } : panel;
  const scale = own ? Math.min(1, room.width / page.width, room.height / page.height) : 1;

  const target = (): Viewport => ({
    width: Math.max(1, Math.round(page.width)),
    height: Math.max(1, Math.round(page.height)),
    deviceScaleFactor: device === "mobile" ? MOBILE.deviceScaleFactor : window.devicePixelRatio || 1,
    mobile: device === "mobile",
  });
  const targetRef = useRef(target);
  targetRef.current = target;

  useLayoutEffect(() => {
    const el = box.current!;
    const measure = () => {
      const r = el.getBoundingClientRect();
      setPanel((p) => (p.width === Math.round(r.width) && p.height === Math.round(r.height) ? p : { width: Math.round(r.width), height: Math.round(r.height) }));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const cv = canvas.current!;
    const ctx = cv.getContext("2d")!;
    let disposed = false;
    let decoding = false;
    let pending: ArrayBuffer | null = null;
    const attachId = crypto.randomUUID();
    attached.current = false;

    const draw = async (buf: ArrayBuffer) => {
      if (decoding) {
        pending = buf; // keep only the newest frame
        return;
      }
      decoding = true;
      try {
        const bitmap = await createImageBitmap(new Blob([buf], { type: "image/jpeg" }));
        if (cv.width !== bitmap.width || cv.height !== bitmap.height) {
          cv.width = bitmap.width;
          cv.height = bitmap.height;
        }
        ctx.drawImage(bitmap, 0, 0);
        bitmap.close();
      } finally {
        decoding = false;
        if (pending && !disposed) {
          const next = pending;
          pending = null;
          void draw(next);
        }
      }
    };

    const attach = async () => {
      viewport.current = targetRef.current();
      const channel = new Channel<ArrayBuffer>();
      channel.onmessage = (buf) => !disposed && void draw(buf);
      await invoke("browser_attach", { workspaceId, targetId: tabId, viewport: viewport.current, attachId, onFrame: channel });
      if (disposed) return;
      attached.current = true;
      // The size may have changed (panel opening, an agent) during the attach.
      syncRef.current();
    };
    void attach().catch((err) => console.error("browser_attach failed", err));

    return () => {
      disposed = true;
      attached.current = false;
      void invoke("browser_detach", { workspaceId, attachId });
    };
  }, [workspaceId, tabId, device]);

  /** Sends the page size to the browser when it changed. */
  const sync = () => {
    const next = targetRef.current();
    const v = viewport.current;
    if (!attached.current || (v && v.width === next.width && v.height === next.height && v.deviceScaleFactor === next.deviceScaleFactor && v.mobile === next.mobile)) return;
    viewport.current = next;
    void invoke("browser_resize", { workspaceId, targetId: tabId, viewport: next });
  };
  const syncRef = useRef(sync);
  syncRef.current = sync;

  useEffect(() => {
    if (page.width < 1 || page.height < 1) return;
    const timer = window.setTimeout(() => syncRef.current(), 50);
    return () => clearTimeout(timer);
  }, [page.width, page.height]);

  /** Drags the page's right edge, bottom edge or corner. The page stays centered, so it grows by twice the drag. */
  const dragEdge = (axes: { x?: boolean; y?: boolean }) => (e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const start = { x: e.clientX, y: e.clientY, width: page.width, height: page.height, scale };
    const move = (ev: PointerEvent) =>
      setPageSize(tabId, {
        width: axes.x ? clamp(start.width + (2 * (ev.clientX - start.x)) / start.scale) : start.width,
        height: axes.y ? clamp(start.height + (2 * (ev.clientY - start.y)) / start.scale) : start.height,
      });
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.style.cursor = "";
      document.body.classList.remove("resizing");
    };
    document.body.style.cursor = axes.x && axes.y ? "nwse-resize" : axes.x ? "ew-resize" : "ns-resize";
    document.body.classList.add("resizing");
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // --- input ---------------------------------------------------------------------------------

  const send = (method: string, params: Record<string, unknown>) =>
    void invoke("browser_input", { workspaceId, targetId: tabId, method, params }).catch(() => {});

  /** Maps a mouse position to page CSS pixels. */
  const point = (e: { clientX: number; clientY: number }) => {
    const r = canvas.current!.getBoundingClientRect();
    const v = viewport.current ?? { width: r.width, height: r.height };
    return { x: ((e.clientX - r.left) / r.width) * v.width, y: ((e.clientY - r.top) / r.height) * v.height };
  };

  const mouse = (type: string) => (e: React.MouseEvent) => {
    if (type === "mousePressed") box.current?.focus();
    send("Input.dispatchMouseEvent", {
      type,
      ...point(e),
      button: type === "mouseMoved" ? (e.buttons & 1 ? "left" : "none") : BUTTONS[e.button] ?? "left",
      buttons: e.buttons,
      clickCount: type === "mouseMoved" ? 0 : e.detail || 1,
      modifiers: modifiers(e),
    });
  };

  let moveFrame = 0;
  const onMove = (e: React.MouseEvent) => {
    cancelAnimationFrame(moveFrame);
    const ev = { clientX: e.clientX, clientY: e.clientY, buttons: e.buttons, altKey: e.altKey, ctrlKey: e.ctrlKey, metaKey: e.metaKey, shiftKey: e.shiftKey };
    moveFrame = requestAnimationFrame(() =>
      send("Input.dispatchMouseEvent", { type: "mouseMoved", ...point(ev), button: ev.buttons & 1 ? "left" : "none", buttons: ev.buttons, modifiers: modifiers(ev) }),
    );
  };

  useEffect(() => {
    const el = box.current!;
    // Wheel must be non-passive to stop the app from scrolling.
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      send("Input.dispatchMouseEvent", { type: "mouseWheel", ...point(e), deltaX: e.deltaX, deltaY: e.deltaY, modifiers: modifiers(e) });
    };
    const onKey = (e: KeyboardEvent) => {
      const down = e.type === "keydown";
      if (e.metaKey || (e.ctrlKey && !navigator.userAgent.includes("Mac"))) {
        const k = e.key.toLowerCase();
        if (down && k === "c") {
          e.preventDefault();
          void invoke<string>("browser_selection", { workspaceId, targetId: tabId }).then((text) => {
            if (text) void navigator.clipboard.writeText(text);
          });
        } else if (down && (k === "a" || k === "z" || k === "x")) {
          e.preventDefault();
          const command = { a: "selectAll", z: e.shiftKey ? "redo" : "undo", x: "cut" }[k];
          send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: e.key, code: e.code, modifiers: modifiers(e), commands: [command] });
        }
        return; // ⌘V arrives as a paste event; other chords belong to the app.
      }
      e.preventDefault();
      const base = { key: e.key, code: e.code, windowsVirtualKeyCode: keyCode(e), modifiers: modifiers(e) };
      if (!down) send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
      else if (e.key === "Enter") send("Input.dispatchKeyEvent", { type: "keyDown", ...base, text: "\r" });
      else if (e.key.length === 1) send("Input.dispatchKeyEvent", { type: "keyDown", ...base, text: e.key });
      else send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...base });
    };
    const onPaste = (e: ClipboardEvent) => {
      const text = e.clipboardData?.getData("text");
      if (text) {
        e.preventDefault();
        send("Input.insertText", { text });
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("keydown", onKey);
    el.addEventListener("keyup", onKey);
    el.addEventListener("paste", onPaste);
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("keydown", onKey);
      el.removeEventListener("keyup", onKey);
      el.removeEventListener("paste", onPaste);
    };
  });

  const handle = "absolute rounded-full bg-border-strong opacity-70 transition-colors hover:bg-accent hover:opacity-100";
  return (
    <div
      ref={box}
      tabIndex={0}
      className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden outline-none"
      style={{ background: own ? "var(--color-app)" : "white" }}
    >
      <div className="relative shrink-0" style={own ? { width: page.width * scale, height: page.height * scale } : { width: "100%", height: "100%" }}>
        <canvas
          ref={canvas}
          data-winfer-capture="browser"
          onMouseDown={mouse("mousePressed")}
          onMouseUp={mouse("mouseReleased")}
          onMouseMove={onMove}
          onContextMenu={(e) => e.preventDefault()}
          className={cn("block h-full w-full bg-white", own && "ring-1 ring-border-strong", device === "mobile" && "rounded-xl")}
        />
        {own && (
          <>
            <div title="Drag to resize · double-click to fit the panel" onPointerDown={dragEdge({ x: true })} onDoubleClick={() => setPageSize(tabId, null)} className={cn(handle, "top-1/2 -right-2.5 h-10 w-1.5 -translate-y-1/2 cursor-ew-resize")} />
            <div title="Drag to resize · double-click to fit the panel" onPointerDown={dragEdge({ y: true })} onDoubleClick={() => setPageSize(tabId, null)} className={cn(handle, "-bottom-2.5 left-1/2 h-1.5 w-10 -translate-x-1/2 cursor-ns-resize")} />
            <div title="Drag to resize · double-click to fit the panel" onPointerDown={dragEdge({ x: true, y: true })} onDoubleClick={() => setPageSize(tabId, null)} className={cn(handle, "-right-2.5 -bottom-2.5 size-2.5 cursor-nwse-resize rounded-sm")} />
          </>
        )}
      </div>
    </div>
  );
}
