import { describe, expect, it, vi } from "vitest";
import { BaseClient } from "./client";
import { MockClient } from "./mock";
import type { DesktopMsg, Message } from "./protocol";

class Client extends BaseClient {
  name = "test";
  request = vi.fn<(_: unknown) => Promise<DesktopMsg>>();
  reconnect() {}
  close() {}
  receive(messages: Message[]) { this.apply({ t: "thread", paneId: "p", source: "winfer", messages }); }
}
const user = (id: string, text = "hello"): Message => ({ id, role: "user", text, ts: null });
const messages = (client: Client) => client.getState().threads.p.messages;

describe("optimistic messages", () => {
  it("appears before acknowledgement and survives stale snapshots until its own echo", async () => {
    const client = new Client();
    client.receive([user("old")]);
    let ack!: (msg: DesktopMsg) => void;
    client.request.mockImplementation(() => new Promise((resolve) => { ack = resolve; }));
    const sending = client.sendMessage("p", "hello");
    expect(messages(client)).toHaveLength(2);
    client.receive([user("old")]);
    expect(messages(client)).toHaveLength(2);
    client.receive([user("old"), user("new")]);
    expect(messages(client).map((m) => m.id)).toEqual(["old", "new"]);
    ack({ t: "ack", id: "1", ok: true });
    await sending;
  });

  it("rolls back on rejection without removing existing messages", async () => {
    const client = new Client();
    client.receive([user("old")]);
    client.request.mockResolvedValue({ t: "ack", id: "1", ok: false, error: "Agent is gone" });
    const sending = client.sendMessage("p", "hello");
    expect(messages(client)).toHaveLength(2);
    await expect(sending).rejects.toThrow("Agent is gone");
    expect(messages(client)).toEqual([user("old")]);
  });

  it("reconciles identical consecutive sends one echo at a time", async () => {
    const client = new Client();
    client.receive([]);
    client.request.mockResolvedValue({ t: "ack", id: "1", ok: true });
    await client.sendMessage("p", "hello");
    await client.sendMessage("p", "hello");
    client.receive([user("one")]);
    expect(messages(client)).toHaveLength(2);
    client.receive([user("one")]);
    expect(messages(client)).toHaveLength(2);
    client.receive([user("one"), user("two")]);
    expect(messages(client)).toHaveLength(2);
    expect(messages(client).map((m) => m.id)).toEqual(["one", "two"]);
  });
});

describe("new session model & effort", () => {
  it("lists provider models and applies chosen model and effort on pane.new", async () => {
    const mock = new MockClient();
    const catalog = await mock.request({ t: "models.get", provider: "winfer" });
    expect(catalog.t).toBe("models");
    if (catalog.t !== "models") return;
    expect(catalog.models.some((m) => m.id === "claude/opus")).toBe(true);
    expect(catalog.defaultModel).toBe("claude/sonnet");

    const ack = await mock.request({ t: "pane.new", workspaceId: "w-winfer", provider: "winfer", model: "claude/opus", effort: "max" });
    expect(ack.t).toBe("ack");
    if (ack.t !== "ack" || !ack.paneId) return;
    const pane = mock.getState().panes.find((p) => p.id === ack.paneId);
    expect(pane?.config?.model).toBe("claude/opus");
    expect(pane?.config?.effort).toBe("max");
    mock.close();
  });
});
