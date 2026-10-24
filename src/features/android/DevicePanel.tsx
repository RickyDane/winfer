import { Channel, invoke } from "@tauri-apps/api/core";
import { ChevronLeft, Circle, Power, RefreshCw, Smartphone, Square } from "lucide-react";
import { Spinner } from "@/components/Spinner";
import { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { PanelActions } from "@/features/plugins/PanelActions";
import { useWorkspacesPick } from "@/stores/workspaces";
import { H264Player } from "./h264";

interface Device {
  serial: string;
  state: string;
  model: string;
  emulator: boolean;
}

interface AndroidStatus {
  adb: boolean;
  devices: Device[];
  avds: string[];
}

interface MirrorInfo {
  width: number;
  height: number;
  videoWidth: number;
  videoHeight: number;
}

type Input =
  | { kind: "tap"; x: number; y: number }
  | { kind: "swipe"; x1: number; y1: number; x2: number; y2: number; ms: number }
  | { kind: "text"; text: string }
  | { kind: "key"; code: number };

// Android keycodes.
const KEY = { back: 4, home: 3, recents: 187, power: 26, enter: 66, del: 67, up: 19, down: 20, left: 21, right: 22, tab: 61, escape: 111 };
const KEYS: Record<string, number> = {
  Enter: KEY.enter,
  Backspace: KEY.del,
  ArrowUp: KEY.up,
  ArrowDown: KEY.down,
  ArrowLeft: KEY.left,
  ArrowRight: KEY.right,
  Tab: KEY.tab,
  Escape: KEY.back,
};

const send = (serial: string, input: Input) => void invoke("android_input", { serial, input }).catch(() => {});

/** Live mirror of one device: H.264 via WebCodecs, or screenshots where that isn't available. */
function Mirror({ device }: { device: Device }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [info, setInfo] = useState<MirrorInfo | null>(null);
  const [mode, setMode] = useState<"stream" | "screenshots">(H264Player.supported() ? "stream" : "screenshots");
  const [error, setError] = useState<string | null>(null);
  const [frames, setFrames] = useState(0);
  const [restart, setRestart] = useState(0);
  const typed = useRef("");
  const typeTimer = useRef<number | undefined>(undefined);

  // Video: start the stream and draw decoded frames.
  useEffect(() => {
    if (mode !== "stream") return;
    let cancelled = false;
    setError(null);
    setFrames(0);
    const player = new H264Player(
      (frame) => {
        const c = canvas.current;
        if (c && !cancelled) {
          if (c.width !== frame.displayWidth || c.height !== frame.displayHeight) {
            c.width = frame.displayWidth;
            c.height = frame.displayHeight;
          }
          c.getContext("2d")?.drawImage(frame, 0, 0);
          setFrames((n) => (n < 2 ? n + 1 : n));
        }
        frame.close();
      },
      // A stream this webview can't decode: fall back to screenshots.
      (message) => {
        console.warn("android stream: can't decode, falling back to screenshots:", message);
        if (!cancelled) setMode("screenshots");
      },
    );
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (data) => player.push(new Uint8Array(data));
    invoke<MirrorInfo>("android_mirror_start", { serial: device.serial, onData: channel }).then(
      (i) => !cancelled && setInfo(i),
      (e) => !cancelled && setError(String(e)),
    );
    return () => {
      cancelled = true;
      player.close();
      void invoke("android_mirror_stop", { serial: device.serial });
    };
  }, [device.serial, mode, restart]);

  // Fallback: a screenshot every half second.
  useEffect(() => {
    if (mode !== "screenshots") return;
    let cancelled = false;
    const tick = async () => {
      while (!cancelled) {
        try {
          const url = await invoke<string>("android_screenshot", { serial: device.serial });
          const img = new Image();
          img.src = url;
          await img.decode();
          const c = canvas.current;
          if (c && !cancelled) {
            c.width = img.naturalWidth;
            c.height = img.naturalHeight;
            c.getContext("2d")?.drawImage(img, 0, 0);
            setInfo((i) => i ?? { width: img.naturalWidth, height: img.naturalHeight, videoWidth: img.naturalWidth, videoHeight: img.naturalHeight });
            setFrames(2);
          }
        } catch (e) {
          if (!cancelled) setError(String(e));
        }
        await new Promise((r) => setTimeout(r, 500));
      }
    };
    void tick();
    return () => {
      cancelled = true;
    };
  }, [device.serial, mode]);

  /** Canvas position → device pixels. */
  const toDevice = useCallback(
    (e: { clientX: number; clientY: number }) => {
      const c = canvas.current!;
      const r = c.getBoundingClientRect();
      const w = info?.width ?? c.width;
      const h = info?.height ?? c.height;
      return {
        x: Math.round(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)) * w),
        y: Math.round(Math.min(1, Math.max(0, (e.clientY - r.top) / r.height)) * h),
      };
    },
    [info],
  );

  // A short press is a tap; a drag is a swipe with the same duration.
  const press = useRef<{ x: number; y: number; at: number } | null>(null);
  const onPointerDown = (e: React.PointerEvent) => {
    canvas.current?.focus();
    canvas.current?.setPointerCapture(e.pointerId);
    press.current = { ...toDevice(e), at: performance.now() };
  };
  const onPointerUp = (e: React.PointerEvent) => {
    const start = press.current;
    press.current = null;
    if (!start) return;
    const end = toDevice(e);
    const ms = Math.round(performance.now() - start.at);
    const moved = Math.hypot(end.x - start.x, end.y - start.y);
    if (moved < 12 && ms < 400) send(device.serial, { kind: "tap", x: start.x, y: start.y });
    else send(device.serial, { kind: "swipe", x1: start.x, y1: start.y, x2: end.x, y2: end.y, ms: Math.max(60, ms) });
  };
  const onWheel = (e: React.WheelEvent) => {
    // Scroll by swiping from the pointer.
    const at = toDevice(e);
    const dy = Math.max(-600, Math.min(600, -e.deltaY * 3));
    send(device.serial, { kind: "swipe", x1: at.x, y1: at.y, x2: at.x, y2: Math.max(0, at.y + dy), ms: 120 });
  };

  // Typing: printable keys are batched into `input text`; others map to keycodes.
  const flushText = () => {
    if (typed.current) send(device.serial, { kind: "text", text: typed.current });
    typed.current = "";
  };
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.metaKey || e.ctrlKey) return; // app shortcuts and paste
    if (e.key.length === 1) {
      e.preventDefault();
      typed.current += e.key;
      clearTimeout(typeTimer.current);
      typeTimer.current = window.setTimeout(flushText, 120);
    } else if (KEYS[e.key]) {
      e.preventDefault();
      flushText();
      send(device.serial, { kind: "key", code: KEYS[e.key] });
    }
  };
  const onPaste = (e: React.ClipboardEvent) => {
    const text = e.clipboardData.getData("text/plain");
    if (text) send(device.serial, { kind: "text", text });
  };

  const key = (code: number) => send(device.serial, { kind: "key", code });

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="relative flex min-h-0 flex-1 items-center justify-center bg-app p-3">
        <canvas
          ref={canvas}
          data-winfer-capture="android"
          tabIndex={0}
          onPointerDown={onPointerDown}
          onPointerUp={onPointerUp}
          onWheel={onWheel}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          className={cn("max-h-full max-w-full rounded-[18px] bg-black shadow-lg outline-none focus:ring-2 focus:ring-accent/50", frames === 0 && "invisible")}
          style={{ aspectRatio: info ? `${info.videoWidth} / ${info.videoHeight}` : undefined }}
        />
        {frames === 0 && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 p-6 text-center text-xs text-fg-subtle">
            {error ? (
              <>
                <span className="text-danger">{error}</span>
                <button onClick={() => setRestart((n) => n + 1)} className="rounded-md border border-border px-2 py-1 text-fg-muted hover:bg-hover hover:text-fg">
                  Try again
                </button>
              </>
            ) : (
              <>
                <Spinner size={16} />
                Connecting to {device.model}… (the picture appears when the screen changes)
              </>
            )}
          </div>
        )}
      </div>
      <div className="flex shrink-0 items-center justify-center gap-1 border-t border-border py-1.5">
        <button title="Back" onClick={() => key(KEY.back)} className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-fg">
          <ChevronLeft size={15} />
        </button>
        <button title="Home" onClick={() => key(KEY.home)} className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-fg">
          <Circle size={13} />
        </button>
        <button title="Recent apps" onClick={() => key(KEY.recents)} className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-fg">
          <Square size={12} />
        </button>
        <span className="mx-1 h-4 w-px bg-border" />
        <button title="Power (screen on/off)" onClick={() => key(KEY.power)} className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-fg">
          <Power size={13} />
        </button>
        <button title="Reconnect the stream" onClick={() => setRestart((n) => n + 1)} className="rounded p-1.5 text-fg-subtle hover:bg-hover hover:text-fg">
          <RefreshCw size={13} />
        </button>
        <span className="ml-2 text-[10px] text-fg-subtle" title={mode === "stream" ? "Live H.264 stream" : "This view can't decode the stream; showing screenshots"}>
          {mode === "stream" ? "live" : "screenshots"}
        </span>
      </div>
    </div>
  );
}

/** Android devices and emulators over adb, mirrored in the side panel. */
export function DevicePanel() {
  const { androidSerial, setAndroidSerial } = useWorkspacesPick("androidSerial", "setAndroidSerial");
  const [status, setStatus] = useState<AndroidStatus | null>(null);
  const [starting, setStarting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Devices come and go (plugged in, emulator booted): poll while the panel is open.
  useEffect(() => {
    let cancelled = false;
    const load = () =>
      invoke<AndroidStatus>("android_status").then(
        (s) => !cancelled && setStatus(s),
        (e) => !cancelled && setError(String(e)),
      );
    void load();
    const timer = window.setInterval(load, 3000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const ready = status?.devices.filter((d) => d.state === "device") ?? [];
  const current = ready.find((d) => d.serial === androidSerial) ?? ready[0] ?? null;

  if (!status) {
    return (
      <div className="flex flex-1 items-center justify-center text-xs text-fg-subtle">
        <Spinner size={14} />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-2">
        <Smartphone size={13} className="shrink-0 text-fg-subtle" />
        {ready.length > 0 ? (
          <select
            value={current?.serial ?? ""}
            onChange={(e) => setAndroidSerial(e.target.value)}
            className="h-7 min-w-0 flex-1 rounded-md border border-border bg-panel px-1.5 text-xs text-fg"
          >
            {ready.map((d) => (
              <option key={d.serial} value={d.serial}>
                {d.model} {d.emulator ? "(emulator)" : ""} · {d.serial}
              </option>
            ))}
          </select>
        ) : (
          <span className="flex-1 text-xs text-fg-muted">No device</span>
        )}
        <PanelActions panel="android" />
      </div>

      {current ? (
        <Mirror key={current.serial} device={current} />
      ) : (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center text-xs text-fg-muted">
          {!status.adb ? (
            <p>
              adb isn't installed. Install Android Studio's platform-tools (macOS: <span className="font-mono">brew install android-platform-tools</span>).
            </p>
          ) : (
            <>
              <p>Connect a phone with USB debugging on, or start an emulator.</p>
              {status.devices
                .filter((d) => d.state !== "device")
                .map((d) => (
                  <p key={d.serial} className="text-warning">
                    {d.serial}: {d.state === "unauthorized" ? "allow USB debugging on the phone" : d.state}
                  </p>
                ))}
              {status.avds.length > 0 && (
                <div className="w-full max-w-60 space-y-1">
                  {status.avds.map((avd) => (
                    <button
                      key={avd}
                      disabled={starting !== null}
                      onClick={() => {
                        setStarting(avd);
                        setError(null);
                        invoke("android_start_avd", { name: avd })
                          .catch((e) => setError(String(e)))
                          // Booting takes a while; the device list picks it up when it's ready.
                          .finally(() => window.setTimeout(() => setStarting(null), 30_000));
                      }}
                      className="flex w-full items-center gap-2 rounded-md border border-border px-2.5 py-1.5 text-left text-fg hover:bg-hover disabled:opacity-50"
                    >
                      {starting === avd ? <Spinner size={12} /> : <Smartphone size={12} />}
                      <span className="flex-1 truncate">{avd.replace(/_/g, " ")}</span>
                      <span className="text-[10px] text-fg-subtle">{starting === avd ? "booting…" : "Start"}</span>
                    </button>
                  ))}
                </div>
              )}
              {error && <p className="text-danger">{error}</p>}
            </>
          )}
        </div>
      )}
    </div>
  );
}
