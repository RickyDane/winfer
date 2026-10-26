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

describe("background task activity", () => {
  it("records time and activity before any dropdown mounts and preserves them on updates", () => {
    const { store, emit } = setup();
    vi.setSystemTime(0);
    emit({ type: "task", id: "shell", status: "running", last: "starting" });
    const first = store.getState().tasks.shell;
    vi.advanceTimersByTime(65_000);
    emit({ type: "task", id: "shell", last: "ready" });
    emit({ type: "task", id: "shell", last: "ready", status: null });
    const task = store.getState().tasks.shell;
    expect(Date.now() - task.startedAt).toBe(65_000);
    expect(task.status).toBe("running");
    expect(task.recentLines).toEqual(["starting", "ready"]);
    expect(first.recentLines).toEqual(["starting"]);
    emit({ type: "task", id: "shell", status: "done" });
    expect(store.getState().tasks.shell.startedAt).toBe(0);
  });

  it("isolates matching task IDs between sessions and resets activity on clear", () => {
    const a = createCodeStore();
    const b = createCodeStore();
    vi.useFakeTimers();
    vi.setSystemTime(1000);
    a.getState().apply({ type: "task", id: "same", status: "running", last: "first" });
    vi.setSystemTime(9000);
    b.getState().apply({ type: "task", id: "same", status: "running", last: "second" });
    expect(a.getState().tasks.same.startedAt).toBe(1000);
    expect(b.getState().tasks.same).toMatchObject({ startedAt: 9000, recentLines: ["second"] });
    a.getState().apply({ type: "cleared" });
    a.getState().apply({ type: "task", id: "same", status: "running" });
    expect(a.getState().tasks.same).toMatchObject({ startedAt: 9000, recentLines: [] });
  });

  it("keeps only the latest eight distinct activity lines", () => {
    const store = createCodeStore();
    for (let i = 0; i < 10; i++) {
      store.getState().apply({ type: "task", id: "agent", last: `Read ${i}` });
    }
    expect(store.getState().tasks.agent.recentLines).toEqual(
      Array.from({ length: 8 }, (_, i) => `Read ${i + 2}`),
    );
  });
});
