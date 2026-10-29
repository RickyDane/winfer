import { afterEach, describe, expect, it, vi } from "vitest";
import { createCodeStore } from "./store";

afterEach(() => vi.useRealTimers());

describe("plan completion", () => {
  it("timestamps only fully completed, nonempty plans without restarting on repeated updates", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    const store = createCodeStore();
    const todos = [{ content: "Build", status: "completed" as const }];
    store.getState().apply({ type: "todos", todos: [...todos, { content: "Test", status: "pending" }] });
    expect(store.getState().planDoneAt).toBeNull();
    store.getState().apply({ type: "todos", todos });
    expect(store.getState().planDoneAt).toBe(1_000);
    vi.advanceTimersByTime(10_000);
    store.getState().apply({ type: "todos", todos: [...todos] });
    expect(store.getState().planDoneAt).toBe(1_000);
    expect(store.getState().todos).toEqual(todos);

    store.getState().apply({ type: "todos", todos: [{ content: "Test", status: "in_progress" }] });
    expect(store.getState().planDoneAt).toBeNull();
    store.getState().apply({ type: "todos", todos });
    expect(store.getState().planDoneAt).toBe(11_000);
    store.getState().apply({ type: "todos", todos: [] });
    expect(store.getState().planDoneAt).toBeNull();
  });

  it("resets completion when the session is cleared", () => {
    const store = createCodeStore();
    store.getState().apply({ type: "todos", todos: [{ content: "Build", status: "completed" }] });
    store.getState().apply({ type: "cleared" });
    expect(store.getState().planDoneAt).toBeNull();
    expect(store.getState().todos).toEqual([]);
  });
});
