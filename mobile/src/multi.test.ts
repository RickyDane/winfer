import { describe, expect, it, vi } from "vitest";
import { BaseClient } from "./client";
import { forMachine, MultiClient } from "./multi";
import type { DesktopMsg, Pane, PhoneMsg } from "./protocol";

class Fake extends BaseClient {
  request = vi.fn<(msg: PhoneMsg) => Promise<DesktopMsg>>(async (msg) => (msg.t === "pane.new" ? { t: "ack", id: "1", ok: true, paneId: "new" } : { t: "ack", id: "1", ok: true }));
  reconnect = vi.fn();
  fire = vi.fn();
  emit(msg: DesktopMsg) {
    this.apply(msg);
  }
  close() {}
  constructor(readonly name: string) {
    super();
  }
  feed(online: boolean, panes: Pane[]) {
    this.set({ socket: "open", desktopOnline: online });
    this.apply({ t: "welcome", desktop: this.name, version: "1", vapidPublicKey: "" });
    this.apply({ t: "panes", panes, workspaces: [{ id: "w", name: "repo", active: true }], providers: [{ id: this.name, name: this.name }] });
  }
}
const pane = (id: string): Pane => ({ id, title: id, workspace: "repo", workspaceId: "w", provider: null, status: "idle", message: null, updatedAt: 0, request: null, reply: null });

function setup() {
  const a = new Fake("mac");
  const b = new Fake("box");
  const multi = new MultiClient([{ id: "A", name: "a", client: a }, { id: "B", name: "b", client: b }], "A");
  a.feed(true, [pane("p1")]);
  b.feed(false, [pane("p1"), pane("p2")]);
  return { a, b, multi };
}

describe("MultiClient", () => {
  it("lists every machine's panes and workspaces under machine-qualified ids", () => {
    const { multi } = setup();
    const s = multi.getState();
    expect(s.panes.map((p) => [p.id, p.workspaceId, p.machineName])).toEqual([
      ["A~p1", "A~w", "mac"],
      ["B~p1", "B~w", "box"],
      ["B~p2", "B~w", "box"],
    ]);
    expect(s.workspaces.map((w) => w.id)).toEqual(["A~w", "B~w"]);
    expect(s.desktopOnline).toBe(true);
    expect(forMachine(s, "B").desktopOnline).toBe(false);
    expect(forMachine(s, "B").providers.map((p) => p.id)).toEqual(["box"]);
  });

  it("sends each request to its machine with the machine's own id, and names replies back", async () => {
    const { a, b, multi } = setup();
    await multi.request({ t: "approve", paneId: "B~p1" });
    expect(b.request).toHaveBeenCalledWith({ t: "approve", paneId: "p1" });
    expect(a.request).not.toHaveBeenCalled();
    const ack = await multi.request({ t: "pane.new", workspaceId: "B~w" });
    expect(ack).toMatchObject({ paneId: "B~new" });
    await multi.request({ t: "models.get", provider: "box" }, "B");
    expect(b.request).toHaveBeenLastCalledWith({ t: "models.get", provider: "box" });
    await multi.request({ t: "usage.get" });
    expect(a.request).toHaveBeenLastCalledWith({ t: "usage.get" });
  });

  it("resolves a bare pane id from a notification to the machine that lists it", async () => {
    const { b, multi } = setup();
    expect(multi.canonicalId("p2")).toBe("B~p2");
    expect(multi.canonicalId("A~p1")).toBe("A~p1");
    await multi.request({ t: "interrupt", paneId: "p2" });
    expect(b.request).toHaveBeenCalledWith({ t: "interrupt", paneId: "p2" });
  });

  it("gives a bare id both machines list to the primary, whose notifications carry them", async () => {
    const a = new Fake("mac");
    const b = new Fake("box");
    const multi = new MultiClient([{ id: "B", name: "b", client: b }, { id: "A", name: "a", client: a }], "A");
    a.feed(true, [pane("p1")]);
    b.feed(true, [pane("p1")]);
    expect(multi.canonicalId("p1")).toBe("A~p1");
    await multi.request({ t: "approve", paneId: "p1" });
    expect(a.request).toHaveBeenCalledWith({ t: "approve", paneId: "p1" });
    expect(b.request).not.toHaveBeenCalled();
  });

  it("routes side panel traffic to its machine and names its streams and replies back", async () => {
    const { a, b, multi } = setup();
    multi.fire({ t: "term.in", paneId: "side-terminal-B~w", data: "eA==" });
    expect(b.fire).toHaveBeenCalledWith({ t: "term.in", paneId: "side-terminal-w", data: "eA==" });
    b.request.mockResolvedValueOnce({ t: "ack", id: "1", ok: true, value: { workspaceId: "w", terminals: [{ id: "side-terminal-w" }, { id: "t1" }] } });
    const info = await multi.request({ t: "panel.get", workspaceId: "B~w" });
    expect(info).toMatchObject({ value: { workspaceId: "B~w", terminals: [{ id: "side-terminal-B~w" }, { id: "B~t1" }] } });
    const seen: DesktopMsg[] = [];
    multi.onMessage((m) => seen.push(m));
    b.emit({ t: "term.out", paneId: "t1", data: "" });
    b.emit({ t: "panel.frame", feed: "browser", seq: 4, data: "", width: 1, height: 1, workspaceId: "w" });
    expect(seen).toMatchObject([{ paneId: "B~t1" }, { workspaceId: "B~w" }]);
    multi.fire({ t: "panel.frameDone", seq: 4 });
    expect(b.fire).toHaveBeenLastCalledWith({ t: "panel.frameDone", seq: 4 });
    expect(a.fire).not.toHaveBeenCalled();
  });

  it("keeps an unchanged machine's pane objects across another machine's update", () => {
    const { a, multi } = setup();
    const before = multi.getState().panes.find((p) => p.id === "B~p2");
    a.feed(true, [pane("p1"), pane("p9")]);
    expect(multi.getState().panes.find((p) => p.id === "B~p2")).toBe(before);
    expect(multi.getState().panes).toHaveLength(4);
  });
});
