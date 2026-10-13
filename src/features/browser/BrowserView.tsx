import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";
import type { Device } from "./browser";

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

/**
 * Shows one tab via CDP screencast and forwards mouse, wheel and keyboard input.
 * The page is emulated at the panel's CSS size (desktop) or a phone size (mobile, scaled to fit).
 */
export function BrowserView({ workspaceId, tabId, device }: { workspaceId: string; tabId: string; device: Device }) {
  const box = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const viewport = useRef<Viewport | null>(null);

  useEffect(() => {
    const el = box.current!;
    const cv = canvas.current!;
    const ctx = cv.getContext("2d")!;
    let disposed = false;
    let attached = false;
    let decoding = false;
    let pending: ArrayBuffer | null = null;
    const attachId = crypto.randomUUID();

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

    const measure = (): Viewport => {
      if (device === "mobile") return MOBILE;
      const r = el.getBoundingClientRect();
      return { width: Math.max(1, Math.round(r.width)), height: Math.max(1, Math.round(r.height)), deviceScaleFactor: window.devicePixelRatio || 1, mobile: false };
    };

    const attach = async () => {
      viewport.current = measure();
      const channel = new Channel<ArrayBuffer>();
      channel.onmessage = (buf) => !disposed && void draw(buf);
      await invoke("browser_attach", { workspaceId, targetId: tabId, viewport: viewport.current, attachId, onFrame: channel });
      attached = true;
    };

    let resizeTimer: number | undefined;
    const observer = new ResizeObserver(() => {
      if (device === "mobile" || !attached) return;
      clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        viewport.current = measure();
        void invoke("browser_resize", { workspaceId, targetId: tabId, viewport: viewport.current });
      }, 80);
    });
    observer.observe(el);
    void attach().catch((err) => console.error("browser_attach failed", err));

    return () => {
      disposed = true;
      observer.disconnect();
      clearTimeout(resizeTimer);
      void invoke("browser_detach", { workspaceId, attachId });
    };
  }, [workspaceId, tabId, device]);

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

  return (
    <div
      ref={box}
      tabIndex={0}
      className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-white outline-none"
      style={{ background: device === "mobile" ? "var(--color-app)" : "white" }}
    >
      <canvas
        ref={canvas}
        onMouseDown={mouse("mousePressed")}
        onMouseUp={mouse("mouseReleased")}
        onMouseMove={onMove}
        onContextMenu={(e) => e.preventDefault()}
        className={device === "mobile" ? "max-h-full max-w-full rounded-xl border border-border-strong object-contain" : "h-full w-full"}
        style={device === "mobile" ? { aspectRatio: `${MOBILE.width} / ${MOBILE.height}`, height: "100%" } : undefined}
      />
    </div>
  );
}
