import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ContextMenu } from "@/components/ContextMenu";
import { cn } from "@/lib/cn";
import { AgentCursor } from "./AgentCursor";
import { logBrowser, setPageSize, type Device, type PageSize } from "./browser";
import { SelectMenu, type SelectProbe } from "./SelectMenu";
import { viewOf } from "./view";

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

// Windows virtual-key codes of the punctuation keys, by physical key.
const CODE_VK: Record<string, number> = {
  Semicolon: 186, Equal: 187, Comma: 188, Minus: 189, Period: 190, Slash: 191, Backquote: 192,
  BracketLeft: 219, Backslash: 220, BracketRight: 221, Quote: 222, IntlBackslash: 226,
};

/**
 * The key's virtual-key code. Taken from the physical key, not the character: a character's code
 * can be another key's ("." is 46 = Delete, "(" is 40 = ArrowDown), which Chrome then acts on.
 */
const keyCode = (e: KeyboardEvent) => {
  if (KEY_CODES[e.key] !== undefined) return KEY_CODES[e.key];
  if (/^Key[A-Z]$/.test(e.code)) return e.code.charCodeAt(3);
  if (/^Digit\d$/.test(e.code)) return e.code.charCodeAt(5);
  return CODE_VK[e.code] ?? e.keyCode ?? 0;
};

const blobBase64 = (blob: Blob) =>
  new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(",")[1] ?? "");
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });

const base64Blob = (data: string, type: string) => {
  const bin = atob(data);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new Blob([bytes], { type });
};

/** Smallest and largest page size the edges can be dragged to. */
const MIN_SIZE = 200;
const MAX_SIZE = 4096;
/** Room around a sized page for its drag handles. */
const GUTTER = 16;
/** A shown tab that sends no frame this long is shown again, up to `BLANK_RETRIES` times in a row. */
const BLANK_AFTER_MS = 3000;
const BLANK_RETRIES = 3;

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
  /** Why the tab couldn't be shown; bumping `attempt` tries again. */
  const [attachError, setAttachError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  /** Re-attaches in a row that brought no frame; reset by a frame or another tab. */
  const blankRetries = useRef(0);
  useEffect(() => void (blankRetries.current = 0), [workspaceId, tabId]);

  /** Attaching before the panel is measured would size the page 1×1, and its frames would read as an agent's size. */
  const measured = panel.width > 0 && panel.height > 0;
  const own = size ?? (device === "mobile" ? MOBILE : null);
  const page = own ?? panel;
  const room = own ? { width: panel.width - 2 * GUTTER, height: panel.height - 2 * GUTTER } : panel;
  const scale = own ? Math.min(1, room.width / page.width, room.height / page.height) : 1;

  const target = (): Viewport => ({
    width: Math.max(1, Math.round(page.width)),
    height: Math.max(1, Math.round(page.height)),
    deviceScaleFactor: device === "mobile" ? MOBILE.deviceScaleFactor : viewOf(box.current).devicePixelRatio || 1,
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
    // From the element's own window, so it keeps firing when the browser is undocked.
    const observer = new (viewOf(el).ResizeObserver)(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // The browser connection dropped (e.g. the computer slept): show the tab again, which reconnects.
  useEffect(() => {
    const unlisten = listen<string>("browser://disconnected", ({ payload }) => {
      if (payload === workspaceId) setAttempt((a) => a + 1);
    });
    return () => void unlisten.then((off) => off());
  }, [workspaceId]);

  // No screencast while the window is hidden (minimized, the app hidden): nothing would show
  // the frames. Showing the tab again on return starts with a fresh screenshot.
  const [hidden, setHidden] = useState(false);
  useEffect(() => {
    const doc = viewOf(box.current).document;
    const update = () => setHidden(doc.hidden);
    update();
    doc.addEventListener("visibilitychange", update);
    return () => doc.removeEventListener("visibilitychange", update);
  }, []);

  useEffect(() => {
    if (!measured || hidden) return;
    const cv = canvas.current!;
    const ctx = cv.getContext("2d")!;
    let disposed = false;
    let decoding = false;
    let pending: ArrayBuffer | null = null;
    const attachId = crypto.randomUUID();
    let gotFrame = false;
    let watchdog: number | undefined;
    attached.current = false;
    setAttachError(null);

    // Each frame is reported once drawn or skipped: the browser sends the next ones only then,
    // so it doesn't encode and ship frames faster than they can be shown.
    const done = () => void invoke("browser_frame_done", { workspaceId, attachId }).catch(() => {});
    const draw = async (buf: ArrayBuffer) => {
      if (!gotFrame) {
        gotFrame = true;
        blankRetries.current = 0;
        clearTimeout(watchdog);
      }
      if (decoding) {
        if (pending) done();
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
        done();
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
      // No picture at all means the tab isn't painting for us: show it again, a few times.
      if (!gotFrame)
        watchdog = window.setTimeout(() => {
          if (disposed || gotFrame) return;
          if (blankRetries.current >= BLANK_RETRIES) {
            setAttachError("The tab isn't showing anything.");
            return;
          }
          blankRetries.current += 1;
          logBrowser("warn", `tab ${tabId} sent no frame, showing it again (${blankRetries.current}/${BLANK_RETRIES})`);
          setAttempt((a) => a + 1);
        }, BLANK_AFTER_MS);
    };
    void attach().catch((err) => {
      logBrowser("error", `showing tab ${tabId} failed: ${String(err)}`);
      if (!disposed) setAttachError(String(err));
    });

    return () => {
      disposed = true;
      clearTimeout(watchdog);
      attached.current = false;
      void invoke("browser_detach", { workspaceId, attachId });
    };
  }, [workspaceId, tabId, device, attempt, measured, hidden]);

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
    const view = viewOf(e.currentTarget);
    const body = view.document.body;
    const up = () => {
      view.removeEventListener("pointermove", move);
      view.removeEventListener("pointerup", up);
      body.style.cursor = "";
      body.classList.remove("resizing");
    };
    body.style.cursor = axes.x && axes.y ? "nwse-resize" : axes.x ? "ew-resize" : "ns-resize";
    body.classList.add("resizing");
    view.addEventListener("pointermove", move);
    view.addEventListener("pointerup", up);
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

  /** The page's open `<select>`, shown as the panel's own option list (see `SelectMenu`). */
  const [menu, setMenu] = useState<SelectProbe | null>(null);
  /** The pending look for a `<select>` under the last press; a release waits for it to keep the order. */
  const press = useRef<Promise<void> | null>(null);
  /** The last press opened the option list instead of reaching the page, so its release mustn't either. */
  const swallowed = useRef(false);
  /** Keys (by `code`) the page was sent the press of. */
  const held = useRef(new Set<string>());

  const closeMenu = (index?: number) => {
    setMenu(null);
    void invoke("browser_select_pick", { workspaceId, targetId: tabId, index: index ?? null }).catch(() => {});
    box.current?.focus();
  };
  // Another tab's page has no such select.
  useEffect(() => () => setMenu(null), [workspaceId, tabId]);

  /** The right-click menu over an image in the page: where it opened (window and page coordinates) and the image's address. */
  const [imageMenu, setImageMenu] = useState<{ x: number; y: number; page: { x: number; y: number }; src: string } | null>(null);
  useEffect(() => () => setImageMenu(null), [workspaceId, tabId]);

  const onContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    const { clientX, clientY } = e;
    const page = point(e);
    void invoke<{ src: string } | null>("browser_image_at", { workspaceId, targetId: tabId, ...page, withData: false })
      .then((found) => found && setImageMenu({ x: clientX, y: clientY, page, src: found.src }))
      .catch(() => {});
  };

  /** Puts the image at the point on the clipboard. The clipboard item takes a promise so the write stays inside the click's user gesture. */
  const copyImage = (at: { x: number; y: number }) => {
    const view = viewOf(box.current);
    const png = invoke<{ data: string } | null>("browser_image_at", { workspaceId, targetId: tabId, ...at, withData: true }).then((found) => {
      if (!found) throw new Error("no image there");
      return base64Blob(found.data, "image/png");
    });
    void view.navigator.clipboard
      .write([new view.ClipboardItem({ "image/png": png })])
      .catch((err) => logBrowser("error", `copying an image from tab ${tabId} failed: ${String(err)}`));
  };

  const mouse = (type: string) => (e: React.MouseEvent) => {
    if (type === "mousePressed") {
      setImageMenu(null);
      // Focus is ours to place: the default would take it from the option list opened below.
      e.preventDefault();
      box.current?.focus();
    }
    const params = {
      type,
      ...point(e),
      button: type === "mouseMoved" ? (e.buttons & 1 ? "left" : "none") : BUTTONS[e.button] ?? "left",
      buttons: e.buttons,
      clickCount: type === "mouseMoved" ? 0 : e.detail || 1,
      modifiers: modifiers(e),
    };
    // Headless Chrome's native option popup isn't in the screencast: a click on a <select> opens ours.
    if (type === "mousePressed" && e.button === 0) {
      swallowed.current = false;
      press.current = invoke<SelectProbe | null>("browser_select_probe", { workspaceId, targetId: tabId, x: params.x, y: params.y })
        .catch(() => null)
        .then((probe) => {
          if (probe) {
            swallowed.current = true;
            setMenu(probe);
          } else send("Input.dispatchMouseEvent", params);
        });
    } else if (type === "mouseReleased" && press.current) {
      const pending = press.current;
      press.current = null;
      void pending.then(() => swallowed.current || send("Input.dispatchMouseEvent", params));
    } else if (!swallowed.current || type !== "mouseReleased") send("Input.dispatchMouseEvent", params);
  };

  let moveFrame = 0;
  const onMove = (e: React.MouseEvent) => {
    const view = viewOf(box.current);
    view.cancelAnimationFrame(moveFrame);
    const ev = { clientX: e.clientX, clientY: e.clientY, buttons: e.buttons, altKey: e.altKey, ctrlKey: e.ctrlKey, metaKey: e.metaKey, shiftKey: e.shiftKey };
    moveFrame = view.requestAnimationFrame(() =>
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
      // AltGr arrives as Ctrl+Alt on Windows and types characters ("@" on many layouts).
      const altGr = e.getModifierState("AltGraph");
      if (e.metaKey || (e.ctrlKey && !altGr && !navigator.userAgent.includes("Mac"))) {
        const k = e.key.toLowerCase();
        if (down && k === "c") {
          e.preventDefault();
          void invoke<string>("browser_selection", { workspaceId, targetId: tabId }).then((text) => {
            // The focused window's clipboard: the main one refuses while unfocused.
            if (text) void viewOf(el).navigator.clipboard.writeText(text);
          });
        } else if (down && (k === "a" || k === "z" || k === "x")) {
          e.preventDefault();
          const command = { a: "selectAll", z: e.shiftKey ? "redo" : "undo", x: "cut" }[k];
          send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: e.key, code: e.code, modifiers: modifiers(e), commands: [command] });
        }
        return; // ⌘V arrives as a paste event; other chords belong to the app.
      }
      e.preventDefault();
      // A key pressed in the option list and released here (it closed on the press) isn't the page's.
      if (down) held.current.add(e.code);
      else if (!held.current.delete(e.code)) return;
      const base = { key: e.key, code: e.code, windowsVirtualKeyCode: keyCode(e), modifiers: modifiers(e) };
      if (!down) send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
      else if (e.key === "Enter") send("Input.dispatchKeyEvent", { type: "keyDown", ...base, text: "\r" });
      // Option composes characters on macOS ("@" is ⌥L on many layouts); sent as Alt, Chrome would
      // treat it as a shortcut and drop the text. Same for AltGr's Ctrl+Alt.
      else if (e.key.length === 1) send("Input.dispatchKeyEvent", { type: "keyDown", ...base, modifiers: base.modifiers & (altGr ? ~3 : ~1), text: e.key });
      else send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...base });
    };
    const onPaste = (e: ClipboardEvent) => {
      const text = e.clipboardData?.getData("text");
      if (text) {
        e.preventDefault();
        send("Input.insertText", { text });
        return;
      }
      // No text: a copied image (a screenshot, "Copy image") goes to the page as a paste of that file.
      const image = [...(e.clipboardData?.files ?? [])].find((f) => f.type.startsWith("image/"));
      if (!image) return;
      e.preventDefault();
      void blobBase64(image)
        .then((data) => invoke("browser_paste_image", { workspaceId, targetId: tabId, data, mime: image.type }))
        .catch((err) => logBrowser("error", `pasting an image into tab ${tabId} failed: ${String(err)}`));
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
          onContextMenu={onContextMenu}
          className={cn("block h-full w-full bg-white", own && "", device === "mobile" && "rounded-xl")}
        />
        <AgentCursor tabId={tabId} page={page} />
        {menu && (
          <SelectMenu
            probe={menu}
            frame={{ width: canvas.current?.clientWidth || page.width, height: canvas.current?.clientHeight || page.height }}
            viewport={viewport.current ?? page}
            onPick={closeMenu}
            onClose={() => closeMenu()}
          />
        )}
        {imageMenu &&
          createPortal(
            <ContextMenu
              x={imageMenu.x}
              y={imageMenu.y}
              items={[
                { label: "Copy image", run: () => copyImage(imageMenu.page) },
                { label: "Copy image address", disabled: !imageMenu.src || imageMenu.src.startsWith("data:"), run: () => void viewOf(box.current).navigator.clipboard.writeText(imageMenu.src) },
              ]}
              onClose={() => {
                setImageMenu(null);
                box.current?.focus();
              }}
            />,
            viewOf(box.current).document.body,
          )}
        {attachError && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-panel px-8 text-center text-xs text-fg-subtle">
            <span className="text-danger">Couldn't show this tab: {attachError}</span>
            <button
              onClick={() => {
                blankRetries.current = 0;
                setAttempt((n) => n + 1);
              }} className="rounded-md bg-panel-raised px-3 py-1.5 font-medium text-fg hover:bg-hover">
              Try again
            </button>
          </div>
        )}
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
