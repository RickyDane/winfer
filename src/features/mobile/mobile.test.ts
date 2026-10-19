import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  listeners: new Map<string, (e: { payload: unknown }) => void>(),
  invoke: vi.fn(async (_cmd: string, _args?: unknown): Promise<unknown> => null),
  execute: vi.fn(),
  sendText: vi.fn(),
  screen: "",
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: h.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, f: (e: { payload: unknown }) => void) => {
    h.listeners.set(name, f);
    return () => {};
  }),
}));
vi.mock("@/commands/bus", () => ({ execute: h.execute }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ readTerminal: () => h.screen, sendText: h.sendText }));
vi.mock("@/stores/activity", () => ({ logActivity: vi.fn() }));
vi.mock("@/features/usage/usage", async () => {
  const { create } = await import("zustand");
  return { useUsage: create(() => ({ context: {} as Record<string, unknown> })) };
});

import { noteStatusMessage, startMobileBridge } from "./mobile";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { useUsage } from "@/features/usage/usage";

const pane = (over: Partial<Pane> = {}) => ({ id: "p1", workspaceId: "w1", provider: "claude", title: "", cwd: "/", status: "working", agentTitle: "Fix login", customTitle: null, prompt: null, ...over }) as Pane;

function setPanes(panes: Pane[]) {
  useWorkspaces.setState({ workspaces: [{ id: "w1", name: "winfer", rootDir: "/", panes, layout: null, zoomedPaneId: null, presetIndex: 0, useWorktrees: false }] });
}

const request = (msg: Record<string, unknown>) => h.listeners.get("mobile://request")!({ payload: { device: "d1", msg } });
const sent = () => h.invoke.mock.calls.filter(([cmd]) => cmd === "mobile_send").map(([, args]) => args as { device: string | null; msg: Record<string, any> });
const flush = () => new Promise((r) => setTimeout(r, 0));

describe("mobile bridge", () => {
  beforeAll(() => {
    setPanes([pane()]);
    startMobileBridge();
  });
  beforeEach(() => {
    h.invoke.mockClear();
    h.execute.mockClear();
  });

  it("lists panes for the phone", async () => {
    request({ t: "panes.get", id: "1" });
    await flush();
    const [{ device, msg }] = sent();
    expect(device).toBe("d1");
    expect(msg).toMatchObject({ t: "panes", id: "1", panes: [{ id: "p1", title: "Fix login", workspace: "winfer", provider: "claude", status: "working", request: null }] });
  });

  it("types the phone's answer into the pane and acks", async () => {
    request({ t: "send", id: "2", paneId: "p1", text: " yes, go ahead " });
    await flush();
    expect(h.execute).toHaveBeenCalledWith({ type: "pane.sendText", target: { id: "p1" }, text: "yes, go ahead", submit: true });
    expect(sent()[0].msg).toEqual({ t: "ack", id: "2", ok: true });
    request({ t: "decline", id: "3", paneId: "p1" });
    request({ t: "approve", id: "4", paneId: "nope" });
    await flush();
    expect(h.execute).toHaveBeenLastCalledWith({ type: "pane.interrupt", target: { id: "p1" } });
    expect(sent()[2].msg).toMatchObject({ t: "ack", id: "4", ok: false });
  });

  it("reads the thread from the session, else the screen", async () => {
    h.screen = "❯ hello";
    request({ t: "thread.get", id: "5", paneId: "p1" });
    await flush();
    expect(sent()[0].msg).toMatchObject({ t: "thread", id: "5", source: "screen", messages: [{ role: "assistant", text: "```\n❯ hello\n```" }] });

    useUsage.setState({ context: { p1: { usedPercent: 1, tokens: 1, window: 1, at: 0, source: "/t.jsonl" } } });
    h.invoke.mockImplementation(async (cmd) => (cmd === "mobile_thread" ? [{ id: "m", role: "user", text: "hi", ts: null }] : null));
    request({ t: "thread.get", id: "6", paneId: "p1" });
    await flush();
    await flush();
    expect(h.invoke).toHaveBeenCalledWith("mobile_thread", { provider: "claude", source: "/t.jsonl", limit: 200 });
    expect(sent().at(-1)!.msg).toMatchObject({ t: "thread", id: "6", source: "claude", messages: [{ text: "hi" }] });
    h.invoke.mockImplementation(async () => null);
  });

  it("reads a winfer code pane's thread from its own session, not its backend's", async () => {
    setPanes([pane({ provider: "winfer", sessionId: "abc123" })]);
    useUsage.setState({ context: { p1: { usedPercent: 1, tokens: 1, window: 1, at: 0, provider: "claude", source: "/backend.jsonl" } } });
    h.invoke.mockImplementation(async (cmd) => (cmd === "mobile_thread" ? [{ id: "w0", role: "user", text: "fix it", ts: null }] : null));
    request({ t: "thread.get", id: "w", paneId: "p1" });
    await flush();
    await flush();
    expect(h.invoke).toHaveBeenCalledWith("mobile_thread", { provider: "winfer", source: "abc123", limit: 200 });
    expect(h.invoke).not.toHaveBeenCalledWith("mobile_thread", expect.objectContaining({ source: "/backend.jsonl" }));
    expect(sent().at(-1)!.msg).toMatchObject({ t: "thread", id: "w", source: "winfer", messages: [{ text: "fix it" }] });
    h.invoke.mockImplementation(async () => null);
    useUsage.setState({ context: {} });
    setPanes([pane()]);
  });

  it("broadcasts status changes, with the question once it's on screen", async () => {
    noteStatusMessage("p1", "Claude needs your permission to use Bash");
    h.screen = "Bash command\n  rm -rf build\nDo you want to proceed?\n❯ 1. Yes\n  2. No";
    vi.useFakeTimers();
    setPanes([pane({ status: "needs-input" })]);
    expect(sent()).toEqual([]);
    vi.advanceTimersByTime(500);
    vi.useRealTimers();
    const [{ device, msg }] = sent();
    expect(device).toBeNull();
    expect(msg.t).toBe("status");
    expect(msg.pane).toMatchObject({ status: "needs-input", message: "Claude needs your permission to use Bash", request: { kind: "approval" } });
    expect(msg.pane.request.text).toContain("Do you want to proceed?");

    setPanes([]);
    expect(sent().at(-1)!.msg).toEqual({ t: "panes", panes: [] });
  });

  it("sends a question's options and picks one by typing its number", async () => {
    h.screen = "Which state library?\n❯ 1. Zustand\n     Small and hook-based\n  2. Redux Toolkit\n  3. Type something.";
    vi.useFakeTimers();
    setPanes([pane({ id: "p2", status: "needs-input" })]);
    vi.advanceTimersByTime(500);
    expect(sent().at(-1)!.msg.pane.request).toMatchObject({
      kind: "question",
      text: "Which state library?",
      options: [{ n: 1, label: "Zustand", detail: "Small and hook-based" }, { n: 2, label: "Redux Toolkit" }, { n: 3, label: "Type something." }],
    });

    request({ t: "choose", id: "c1", paneId: "p2", option: 2 });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.sendText).toHaveBeenCalledWith("p2", "2");
    expect(sent().at(-1)!.msg).toEqual({ t: "ack", id: "c1", ok: true });

    request({ t: "choose", id: "c2", paneId: "p2", option: 3, text: "Jotai" });
    await vi.advanceTimersByTimeAsync(300);
    expect(h.sendText).toHaveBeenLastCalledWith("p2", "3");
    expect(h.execute).toHaveBeenCalledWith({ type: "pane.sendText", target: { id: "p2" }, text: "Jotai", submit: true });

    request({ t: "choose", id: "c3", paneId: "p2", option: 7 });
    await vi.advanceTimersByTimeAsync(0);
    expect(sent().at(-1)!.msg).toMatchObject({ t: "ack", id: "c3", ok: false });
    vi.useRealTimers();
    setPanes([pane()]);
  });
});
