import { afterEach, describe, expect, it, vi } from "vitest";
import { batchEvents } from "./eventBatcher";

afterEach(() => vi.useRealTimers());

describe("stream event batching", () => {
  it("turns a token burst into one store update and drains without another event", () => {
    vi.useFakeTimers();
    const apply = vi.fn();
    const events = batchEvents(apply);
    for (let i = 0; i < 100; i++) events.push({ type: "text_delta", id: "a", text: "x" });
    expect(apply).not.toHaveBeenCalled();
    vi.advanceTimersByTime(16);
    expect(apply.mock.calls).toEqual([[{ type: "text_delta", id: "a", text: "x".repeat(100) }]]);
  });

  it("preserves message, reasoning and tool ordering and flushes before control events", () => {
    vi.useFakeTimers();
    const apply = vi.fn();
    const events = batchEvents(apply);
    events.push({ type: "reasoning_delta", id: "r", text: "think" });
    events.push({ type: "reasoning_delta", id: "r", text: "ing" });
    events.push({ type: "text_delta", id: "a", text: "hello" });
    events.push({ type: "text_delta", id: "b", text: "there" });
    events.push({ type: "tool_output", id: "t", chunk: "one" });
    events.push({ type: "tool_output", id: "t", chunk: "two" });
    events.push({ type: "turn_done" });
    expect(apply.mock.calls.map(([ev]) => ev)).toEqual([
      { type: "reasoning_delta", id: "r", text: "thinking" },
      { type: "text_delta", id: "a", text: "hello" },
      { type: "text_delta", id: "b", text: "there" },
      { type: "tool_output", id: "t", chunk: "onetwo" },
      { type: "turn_done" },
    ]);
    vi.runAllTimers();
    expect(apply).toHaveBeenCalledTimes(5);
  });

  it("doesn't notify or retain a closed pane", () => {
    vi.useFakeTimers();
    const apply = vi.fn();
    const events = batchEvents(apply);
    events.push({ type: "text_delta", id: "a", text: "x" });
    events.dispose();
    events.push({ type: "turn_done" });
    vi.runAllTimers();
    expect(apply).not.toHaveBeenCalled();
  });
});
