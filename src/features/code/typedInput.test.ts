import { describe, expect, it, vi } from "vitest";
import { createCodeStore } from "./store";
import { typeIntoCode } from "./typedInput";
import type { Approval } from "./protocol";

function setup() {
  const store = createCodeStore();
  const send = vi.fn();
  store.getState().attach({ connect: () => () => {}, send });
  return { store, send };
}

const approval: Approval = { id: "a1", tool: "Bash", title: "rm x", detail: "", questions: [] };

describe("typing into a winfer code pane", () => {
  it("sends typed text on Enter, like the agent bar's broadcast", () => {
    const { store, send } = setup();
    typeIntoCode(store, "Write a story");
    typeIntoCode(store, "\r");
    expect(send).toHaveBeenCalledWith({ type: "send", text: "Write a story" });
    expect(store.getState().draft).toBe("");
  });

  it("leaves text in the composer without Enter (Type only)", () => {
    const { store, send } = setup();
    typeIntoCode(store, "draft me");
    expect(store.getState().draft).toBe("draft me");
    expect(send).not.toHaveBeenCalled();
  });

  it("pastes multi-line text as one message", () => {
    const { store, send } = setup();
    typeIntoCode(store, "\x1b[200~line 1\nline 2\x1b[201~");
    typeIntoCode(store, "\r");
    expect(send).toHaveBeenCalledWith({ type: "send", text: "line 1\nline 2" });
  });

  it("interrupts on Esc and approves on y", () => {
    const { store, send } = setup();
    typeIntoCode(store, "\x1b");
    expect(send).toHaveBeenLastCalledWith({ type: "interrupt" });
    store.setState({ approvals: [approval] });
    typeIntoCode(store, "y");
    expect(send).toHaveBeenLastCalledWith({ type: "approve", id: "a1", decision: "allow" });
  });

  it("ignores Enter on an empty composer", () => {
    const { store, send } = setup();
    typeIntoCode(store, "\r");
    expect(send).not.toHaveBeenCalled();
  });
});
