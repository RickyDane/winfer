import { describe, expect, it } from "vitest";
import { createCodeStore } from "@/features/code/store";
import { liveThread } from "./liveThread";

describe("live mobile transcript", () => {
  it("includes user messages and streamed text before a turn completes", () => {
    const store = createCodeStore();
    store.getState().submit("Fix it");
    store.getState().apply({ type: "text_delta", id: "a", text: "Checking " });
    expect(liveThread(store.getState(), 200)).toMatchObject([
      { role: "user", text: "Fix it" }, { id: "a", role: "assistant", text: "Checking " },
    ]);
    store.getState().apply({ type: "text_delta", id: "a", text: "now" });
    expect(liveThread(store.getState(), 1)).toMatchObject([{ id: "a", text: "Checking now" }]);
  });

  it("updates tools in place and exposes live reasoning without its private text", () => {
    const store = createCodeStore();
    store.getState().apply({ type: "tool_start", id: "t", name: "Bash", title: "Run tests", input: {} });
    expect(liveThread(store.getState(), 200)).toMatchObject([{ id: "t", state: "running" }]);
    store.getState().apply({ type: "tool_end", id: "t", ok: false, output: "failed" });
    expect(liveThread(store.getState(), 200)).toMatchObject([{ id: "t", state: "failed" }]);
    store.setState({ busy: true });
    store.getState().apply({ type: "reasoning_delta", id: "r", text: "private" });
    expect(liveThread(store.getState(), 1)).toMatchObject([{ id: "r", role: "thinking", text: "" }]);
  });
});
