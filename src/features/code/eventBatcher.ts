import type { Ev } from "./protocol";

type Delta = Extract<Ev, { type: "text_delta" | "reasoning_delta" | "tool_output" }>;
const isDelta = (ev: Ev): ev is Delta =>
  ev.type === "text_delta" || ev.type === "reasoning_delta" || ev.type === "tool_output";

/** Coalesce token bursts before copying the transcript and notifying its subscribers.
 * A timer (not rAF) also drains in hidden panes. Control events always flush first. */
export function batchEvents(apply: (ev: Ev) => void) {
  let pending: Delta | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;
  const flush = () => {
    clearTimeout(timer);
    timer = undefined;
    const ev = pending;
    pending = undefined;
    if (ev) apply(ev);
  };
  return {
    push(ev: Ev) {
      if (disposed) return;
      if (!isDelta(ev)) {
        flush();
        apply(ev);
        return;
      }
      if (pending?.type !== ev.type || pending.id !== ev.id) flush();
      if (pending?.type === "tool_output" && ev.type === "tool_output")
        pending = { ...pending, chunk: pending.chunk + ev.chunk };
      else if (pending && "text" in pending && "text" in ev)
        pending = { ...pending, text: pending.text + ev.text };
      else pending = ev;
      timer ??= setTimeout(flush, 16);
    },
    dispose() {
      disposed = true;
      clearTimeout(timer);
      pending = undefined;
    },
  };
}
