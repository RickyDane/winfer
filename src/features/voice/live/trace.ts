/**
 * Timing log for live voice (written to live-voice.log in the app data folder by
 * src-tauri/src/realtime.rs): each step with the time since the previous one and the wall
 * clock, so a stall shows as a large gap after the step that caused it.
 */
import { invoke } from "@tauri-apps/api/core";

let last = performance.now();

export function trace(step: string, detail?: unknown) {
  const now = performance.now();
  const gap = Math.round(now - last);
  last = now;
  const extra = detail === undefined ? "" : ` ${typeof detail === "string" ? detail : JSON.stringify(detail)}`.slice(0, 400);
  const line = `${new Date().toISOString()} +${gap}ms ${step}${extra}`;
  void invoke("live_voice_log", { line }).catch(() => {});
}

/** Times an awaited step: logs when it starts and how long it took (or how it failed). */
export async function timed<T>(step: string, run: () => Promise<T>): Promise<T> {
  trace(`${step}…`);
  const started = performance.now();
  try {
    const out = await run();
    trace(`${step} done`, `${Math.round(performance.now() - started)}ms`);
    return out;
  } catch (err) {
    trace(`${step} failed`, `${Math.round(performance.now() - started)}ms ${err instanceof Error ? `${err.name}: ${err.message}` : String(err)}`);
    throw err;
  }
}
