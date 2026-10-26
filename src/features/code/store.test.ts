import { afterEach, describe, expect, it, vi } from "vitest";
import { createCodeStore } from "./store";
import type { Ev, Transport } from "./protocol";

afterEach(() => vi.useRealTimers());

function setup() {
  vi.useFakeTimers();
  let emit!: (ev: Ev) => void;
  const disconnect = vi.fn();
  const transport: Transport = {
    connect: (onEvent) => { emit = onEvent; return disconnect; },
    send: vi.fn(),
  };
  const store = createCodeStore();
  const detach = store.getState().attach(transport);
  return { store, emit, detach, disconnect };
}

describe("batched code transport", () => {
  it("preserves immutable historical rows while reducing subscriber notifications", () => {
    const { store, emit } = setup();
    emit({ type: "text", id: "old", text: "history" });
    const previous = store.getState().items[0];
    const changed = vi.fn();
    store.subscribe(changed);
    for (let i = 0; i < 500; i++) emit({ type: "text_delta", id: "new", text: "." });
    vi.advanceTimersByTime(16);
    expect(changed).toHaveBeenCalledOnce();
    expect(store.getState().items[0]).toBe(previous);
    expect(store.getState().items[1]).toMatchObject({ text: ".".repeat(500), streaming: true });
  });

  it("flushes tokens before completion and clears without resurrecting buffered text", () => {
    const { store, emit } = setup();
    emit({ type: "text_delta", id: "a", text: "partial" });
    emit({ type: "text", id: "a", text: "complete" });
    expect(store.getState().items).toHaveLength(1);
    expect(store.getState().items[0]).toMatchObject({ text: "complete", streaming: false });
    emit({ type: "text_delta", id: "b", text: "discard" });
    emit({ type: "cleared" });
    vi.runAllTimers();
    expect(store.getState().items).toEqual([]);
  });

  it("disconnects and cancels pending updates when a pane closes", () => {
    const { store, emit, detach, disconnect } = setup();
    emit({ type: "text_delta", id: "a", text: "pending" });
    detach();
    emit({ type: "notice", text: "late event" });
    vi.runAllTimers();
    expect(disconnect).toHaveBeenCalledOnce();
    expect(store.getState().items).toEqual([]);
  });
});
