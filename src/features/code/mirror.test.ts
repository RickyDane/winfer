import { describe, expect, it } from "vitest";
import { createCodeStore } from "./store";
import type { Cmd, Ev, Transport } from "./protocol";

/** A session host's store and a phone's mirror of it, wired like the relay does. */
function pair() {
  const sent: Cmd[] = [];
  let emit: (ev: Ev) => void = () => {};
  const host = createCodeStore();
  host.getState().attach({ connect: (on) => ((emit = on), () => {}), send: (c) => sent.push(c) } satisfies Transport);
  const phone = createCodeStore({ remote: true });
  phone.getState().load(JSON.parse(JSON.stringify(host.getState().snapshot())));
  host.getState().listen((ev) => phone.getState().apply(ev));
  // The phone's commands run through the host's store, as codeBridge.command does.
  phone.getState().attach({
    connect: () => () => {},
    send: (c) => (c.type === "send" || c.type === "steer" ? host.getState().submit(c.text) : host.getState().send(c)),
  });
  return { host, phone, sent, emit: (ev: Ev) => emit(ev) };
}

describe("mirroring a winfer code session", () => {
  it("shows the phone's turn once on each side and sends it to the session", () => {
    const { host, phone, sent } = pair();
    phone.getState().submit("hello");
    const users = (s: typeof host) => s.getState().items.filter((i) => i.kind === "user").map((i) => (i as { text: string }).text);
    expect(users(host)).toEqual(["hello"]);
    expect(users(phone)).toEqual(["hello"]);
    expect(sent).toEqual([{ type: "send", text: "hello" }]);
  });

  it("follows the session's events and the host's own changes", () => {
    const { host, phone, emit } = pair();
    emit({ type: "turn_start" });
    emit({ type: "text_delta", id: "a", text: "Hi" });
    emit({ type: "approval", id: "p1", tool: "Bash", title: "ls", input: {} } as unknown as Ev);
    host.getState().send({ type: "set_mode", mode: "plan" });
    host.getState().send({ type: "set_effort", effort: "low" });
    phone.getState().send({ type: "approve", id: "p1", decision: "allow" });
    const s = phone.getState();
    expect(s.busy).toBe(true);
    expect(s.items.some((i) => i.kind === "text" && i.text === "Hi")).toBe(true);
    expect(s.mode).toBe("plan");
    expect(s.effort).toBe("low");
    expect(s.approvals).toEqual([]);
    expect(host.getState().approvals).toEqual([]);
  });

  it("catches up from a snapshot", () => {
    const { host, emit } = pair();
    emit({ type: "text", id: "t", text: "done" });
    const late = createCodeStore({ remote: true });
    late.getState().load(JSON.parse(JSON.stringify(host.getState().snapshot())));
    expect(late.getState().items).toEqual(host.getState().items);
  });
});
