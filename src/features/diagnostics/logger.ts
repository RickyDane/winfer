/**
 * Frontend side of the diagnostics log (see src-tauri/src/diagnostics.rs).
 *
 * Everything important in the webview — uncaught errors, unhandled promise
 * rejections, React render crashes, `console.error/warn`, and a periodic
 * webview-memory sample — is batched and forwarded to the Rust file log, so
 * it survives the crash or reload that follows it. In the plain-browser mock
 * (`pnpm dev`) entries go to the console instead.
 *
 * Batching keeps this off hot paths: entries queue up and flush every 2 s,
 * at 25 entries, on `error`, and when the page hides. A flood is capped —
 * a loop logging errors must not grow the queue without bound.
 */
import { invoke } from "@tauri-apps/api/core";

export type LogLevel = "error" | "warn" | "info" | "debug";

interface Entry {
  level: LogLevel;
  target: string;
  message: string;
}

const MAX_QUEUE = 200;
const FLUSH_EVERY_MS = 2_000;
const WEBVIEW_SAMPLE_EVERY_MS = 30_000;

let queue: Entry[] = [];
let flushTimer: number | undefined;
let dropped = 0;

/** Extra context (pane/terminal counts, …) included in memory samples. */
let contextProvider: (() => Record<string, number | string>) | null = null;
export function setLogContext(provider: () => Record<string, number | string>) {
  contextProvider = provider;
}

function inTauri() {
  return "__TAURI_INTERNALS__" in window;
}

function format(args: unknown[]): string {
  return args
    .map((a) => {
      if (a instanceof Error) return a.stack ?? `${a.name}: ${a.message}`;
      if (typeof a === "string") return a;
      try {
        return JSON.stringify(a);
      } catch {
        return String(a);
      }
    })
    .join(" ")
    .slice(0, 4096);
}

export function log(level: LogLevel, target: string, ...args: unknown[]) {
  if (queue.length >= MAX_QUEUE) {
    dropped++;
    return;
  }
  queue.push({ level, target, message: format(args) });
  if (level === "error" || queue.length >= 25) void flushLog();
  else scheduleFlush();
}

export const logger = {
  error: (target: string, ...args: unknown[]) => log("error", target, ...args),
  warn: (target: string, ...args: unknown[]) => log("warn", target, ...args),
  info: (target: string, ...args: unknown[]) => log("info", target, ...args),
  debug: (target: string, ...args: unknown[]) => log("debug", target, ...args),
  /** A short "what just happened" note; Rust keeps the last 200 for crash reports. */
  breadcrumb: (message: string) => log("info", "breadcrumb", message),
};

function scheduleFlush() {
  if (flushTimer !== undefined) return;
  flushTimer = window.setTimeout(() => {
    flushTimer = undefined;
    void flushLog();
  }, FLUSH_EVERY_MS);
}

export async function flushLog() {
  if (flushTimer !== undefined) {
    clearTimeout(flushTimer);
    flushTimer = undefined;
  }
  if (!queue.length && !dropped) return;
  const entries = queue;
  queue = [];
  if (dropped) {
    entries.push({ level: "warn", target: "logger", message: `${dropped} log entries dropped (queue full)` });
    dropped = 0;
  }
  if (!inTauri()) {
    for (const e of entries) console.debug(`[winfer:${e.level}] ${e.target}: ${e.message}`);
    return;
  }
  try {
    await invoke("diagnostics_frontend_log", { entries });
  } catch {
    // The backend is gone (or this is an old binary): never recurse into log.
  }
}

interface MemoryInfo {
  usedJSHeapSize: number;
  totalJSHeapSize: number;
  jsHeapSizeLimit: number;
}

/** Webview JS heap (Chromium only) + page context, for the memory timeline. */
function sampleWebviewMemory() {
  const perf = performance as Performance & { memory?: MemoryInfo };
  const ctx = contextProvider?.() ?? {};
  const parts = Object.entries(ctx).map(([k, v]) => `${k}=${v}`);
  if (perf.memory) {
    const mb = (n: number) => `${Math.round(n / 1048576)}MB`;
    parts.push(`jsHeap=${mb(perf.memory.usedJSHeapSize)}/${mb(perf.memory.totalJSHeapSize)}`, `heapLimit=${mb(perf.memory.jsHeapSizeLimit)}`);
  }
  logger.info("webview-memory", parts.join(" ") || "n/a");
}

let installed = false;

/**
 * Installs the global handlers once: window errors, unhandled rejections,
 * console.error/warn forwarding (originals still run), visibility flush and
 * the 30 s webview-memory sample. Call from AppShell startup.
 */
export function installDiagnostics() {
  if (installed) return;
  installed = true;

  window.addEventListener("error", (e) => {
    logger.error("window", e.message, e.error ?? `${e.filename}:${e.lineno}:${e.colno}`);
  });
  window.addEventListener("unhandledrejection", (e) => {
    logger.error("unhandledrejection", e.reason);
  });

  const origError = console.error.bind(console);
  const origWarn = console.warn.bind(console);
  console.error = (...args: unknown[]) => {
    origError(...args);
    log("error", "console", ...args);
  };
  console.warn = (...args: unknown[]) => {
    origWarn(...args);
    log("warn", "console", ...args);
  };

  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") void flushLog();
  });
  window.addEventListener("pagehide", () => void flushLog());

  sampleWebviewMemory();
  window.setInterval(sampleWebviewMemory, WEBVIEW_SAMPLE_EVERY_MS);
  logger.info("app", "webview diagnostics installed", navigator.userAgent);
}

/** For tests: the pending (unflushed) entries. */
export function pendingEntries(): readonly Entry[] {
  return queue;
}
