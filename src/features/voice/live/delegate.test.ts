import { beforeEach, describe, expect, it, vi } from "vitest";

const turns: unknown[][] = [];
const invoke = vi.fn(async (cmd: string, _args?: unknown) => (cmd === "chatgpt_tool_turn" ? turns.shift() : null));
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: unknown) => invoke(cmd, args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@/features/palette/entries", () => ({ toolContext: () => ({ providers: [], workspaces: [], active: { id: "ws", name: "api", rootDir: "~", panes: [], layout: null }, focusedPaneId: null, browserTabs: [] }) }));
vi.mock("./tools", () => ({
  END_CONVERSATION: "end_conversation",
  voiceTools: async () => [{ type: "function", name: "list_panes", description: "List panes", parameters: { type: "object", properties: {} } }],
  runVoiceFunction: vi.fn(async (name: string) => `ran ${name}`),
}));

const { answerText, chunks, Delegate, trimHistory } = await import("./delegate");
const tools = await import("./tools");

const message = (text: string) => ({ type: "message", role: "assistant", content: [{ type: "output_text", text }] });

beforeEach(() => {
  turns.length = 0;
  vi.clearAllMocks();
});

describe("delegate", () => {
  it("runs tool calls until the model answers, keeping the history for follow-ups", async () => {
    turns.push([{ type: "function_call", call_id: "c1", name: "list_panes", arguments: "{}" }], [message("Two panes are open.")]);
    const results: string[] = [];
    const d = new Delegate({ onTool: () => {}, onResult: (name, _a, out) => results.push(`${name}:${out}`) });
    expect(await d.run("what's open?")).toEqual({ text: "Two panes are open.", end: false });
    expect(tools.runVoiceFunction).toHaveBeenCalledWith("list_panes", {}, expect.any(Function));
    expect(results).toEqual(["list_panes:ran list_panes"]);
    const second = invoke.mock.calls.filter(([c]) => c === "chatgpt_tool_turn")[1][1] as { input: { type?: string; call_id?: string }[] };
    expect(second.input.at(-1)).toEqual({ type: "function_call_output", call_id: "c1", output: "ran list_panes" });

    turns.push([message("Okay.")]);
    await d.run("thanks");
    const third = invoke.mock.calls.filter(([c]) => c === "chatgpt_tool_turn")[2][1] as { input: unknown[] };
    expect(third.input).toHaveLength(5);
  });

  it("ends the conversation without running a tool", async () => {
    turns.push([{ type: "function_call", call_id: "c1", name: "end_conversation", arguments: "" }], [message("Bye!")]);
    expect(await new Delegate({ onTool: () => {}, onResult: () => {} }).run("bye")).toEqual({ text: "Bye!", end: true });
    expect(tools.runVoiceFunction).not.toHaveBeenCalled();
  });

  it("reads answers, trims whole requests and splits long text", () => {
    expect(answerText([message("a"), { type: "reasoning" }, message("b")])).toBe("a b");
    const items = [{ role: "user" }, { type: "x" }, { role: "user" }, { type: "y" }, { type: "z" }];
    expect(trimHistory(items, 3)).toEqual(items.slice(2));
    expect(chunks("äbc", 3)).toEqual(["äb", "c"]);
    expect(chunks("hi")).toEqual(["hi"]);
  });
});
