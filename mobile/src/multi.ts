import type { ClientState, DesktopClient, Machine, Thread } from "./client";
import type { DesktopMsg, Pane, PhoneMsg, Workspace } from "./protocol";

/** Joins a machine's id to one of its pane or workspace ids. Device ids are base64url: never a `~`. */
const SEP = "~";
/** A workspace's side terminal is the pane `side-terminal-<workspace id>`: its machine goes on the workspace id. */
const SIDE = "side-terminal-";

export interface Member {
  /** The pairing's device id. */
  id: string;
  name: string;
  client: DesktopClient;
}

/** Several paired machines as one: their panes and workspaces side by side, each request sent to its machine. */
export class MultiClient implements DesktopClient {
  readonly name: string;
  private listeners = new Set<() => void>();
  private unsubs: (() => void)[];
  private streamListeners = new Set<(msg: DesktopMsg) => void>();
  /** The machine whose panel frames are showing: it gets their acknowledgements. */
  private frameFrom: Member | null = null;
  private state: ClientState;
  /** Each member's last mapped lists, so an unchanged machine keeps its objects. */
  private memo = new Map<string, { name: string; src: ClientState; panes: Pane[]; ws: Workspace[]; threads: Record<string, Thread> }>();

  constructor(
    private members: Member[],
    private primaryId: string,
  ) {
    this.name = `${members.length} machines`;
    this.state = this.merge();
    this.unsubs = members.flatMap((m) => [
      m.client.subscribe(() => {
        this.state = this.merge();
        for (const fn of this.listeners) fn();
      }),
      m.client.onMessage((msg) => {
        if (msg.t === "panel.frame") this.frameFrom = m;
        const lifted = this.lift(m, msg);
        for (const fn of this.streamListeners) fn(lifted);
      }),
    ]);
  }

  getState = () => this.state;

  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => void this.listeners.delete(fn);
  };

  private get primary(): Member {
    return this.members.find((m) => m.id === this.primaryId) ?? this.members[0];
  }

  /** Primary first: notifications (the bare ids) come from it, and ids can repeat across machines. */
  private get ordered(): Member[] {
    return [this.primary, ...this.members.filter((m) => m !== this.primary)];
  }

  private nameOf(m: Member): string {
    return m.client.getState().welcome?.desktop || m.name;
  }

  /** The member an id belongs to and the id as that machine knows it. Bare ids (from a notification) are looked up. */
  private split(id: string, kind: "pane" | "workspace"): { member: Member; id: string } {
    if (kind === "pane" && id.startsWith(SIDE)) {
      const { member, id: ws } = this.split(id.slice(SIDE.length), "workspace");
      return { member, id: SIDE + ws };
    }
    const i = id.indexOf(SEP);
    if (i > 0) {
      const member = this.members.find((m) => m.id === id.slice(0, i));
      if (member) return { member, id: id.slice(i + 1) };
    }
    const has = (m: Member) =>
      kind === "pane" ? m.client.getState().panes.some((p) => p.id === id) : m.client.getState().workspaces.some((w) => w.id === id);
    return { member: this.ordered.find(has) ?? this.primary, id };
  }

  /** A pane id as this client names it: a bare one (from a notification) gets its machine once that machine lists it. */
  canonicalId(id: string): string {
    if (id.includes(SEP) && this.members.some((m) => id.startsWith(m.id + SEP))) return id;
    const m = this.ordered.find((x) => x.client.getState().panes.some((p) => p.id === id));
    return m ? m.id + SEP + id : id;
  }

  private merge(): ClientState {
    const primary = this.primary;
    const states = this.members.map((m) => ({ m, s: m.client.getState() }));
    const machines: Machine[] = states.map(({ m, s }) => ({
      id: m.id,
      name: this.nameOf(m),
      primary: m === primary,
      socket: s.socket,
      desktopOnline: s.desktopOnline,
      welcome: s.welcome,
      providers: s.providers,
      defaultProvider: s.defaultProvider,
      error: s.error,
    }));
    const panes: Pane[] = [];
    const workspaces: Workspace[] = [];
    const threads: Record<string, Thread> = {};
    for (const { m, s } of states) {
      const name = this.nameOf(m);
      const q = (id: string) => m.id + SEP + id;
      const was = this.memo.get(m.id);
      const same = was?.name === name;
      const memo = {
        name,
        src: s,
        panes:
          same && was.src.panes === s.panes
            ? was.panes
            : s.panes.map((p) => ({ ...p, id: q(p.id), workspaceId: p.workspaceId && q(p.workspaceId), machine: m.id, machineName: name })),
        ws: same && was.src.workspaces === s.workspaces ? was.ws : s.workspaces.map((w) => ({ ...w, id: q(w.id), machine: m.id, machineName: name })),
        threads: was?.src.threads === s.threads ? was.threads : Object.fromEntries(Object.entries(s.threads).map(([k, v]) => [q(k), v])),
      };
      this.memo.set(m.id, memo);
      panes.push(...memo.panes);
      workspaces.push(...memo.ws);
      Object.assign(threads, memo.threads);
    }
    const all = states.map((x) => x.s);
    const ps = primary.client.getState();
    // Providers: each machine's, once (NewSession shows the chosen workspace's machine's own).
    const providers = [...new Map(all.flatMap((s) => s.providers).map((p) => [p.id, p])).values()];
    return {
      socket: all.some((s) => s.socket === "open") ? "open" : all.some((s) => s.socket === "connecting") ? "connecting" : "closed",
      // Online if any machine is; offline only once every one says so.
      desktopOnline: all.some((s) => s.desktopOnline) ? true : all.every((s) => s.desktopOnline === false) ? false : null,
      welcome: ps.welcome,
      panes,
      workspaces,
      providers,
      defaultProvider: ps.defaultProvider ?? all.find((s) => s.defaultProvider)?.defaultProvider ?? null,
      compactTools: ps.compactTools,
      showBgNotes: ps.showBgNotes,
      threads,
      // Trouble everywhere shows; one machine's shows on its own panes (see `forMachine`).
      error: all.every((s) => s.error) ? ps.error : null,
      machines,
    };
  }

  private liftPane(m: Member, id: string): string {
    return id.startsWith(SIDE) ? SIDE + m.id + SEP + id.slice(SIDE.length) : m.id + SEP + id;
  }

  /** A member's message in this client's terms: its pane and workspace ids get the machine. */
  private lift(m: Member, msg: DesktopMsg): DesktopMsg {
    const ids = <T extends object>(o: T): T => {
      const r = { ...o } as Record<string, unknown>;
      if (typeof r.paneId === "string") r.paneId = this.liftPane(m, r.paneId);
      if (typeof r.workspaceId === "string") r.workspaceId = m.id + SEP + r.workspaceId;
      return r as T;
    };
    if (msg.t === "status" || msg.t === "panes" || msg.t === "welcome") return msg;
    const out = ids(msg);
    // Replies with a value (panel.get, browser/files calls): its workspace and terminals too.
    if (out.t === "ack" && out.value && typeof out.value === "object" && !Array.isArray(out.value)) {
      const v = ids(out.value as Record<string, unknown>);
      if (Array.isArray(v.terminals)) v.terminals = v.terminals.map((t: { id: string }) => ({ ...t, id: this.liftPane(m, t.id) }));
      return { ...out, value: v };
    }
    return out;
  }

  /** Where a message goes, as that machine names things; null: to every machine (pane lists). */
  private route(msg: PhoneMsg, via?: string): { member: Member; msg: PhoneMsg } | null {
    const o = msg as Record<string, unknown>;
    if (typeof o.paneId === "string") {
      const { member, id } = this.split(o.paneId, "pane");
      return { member, msg: { ...msg, paneId: id } as PhoneMsg };
    }
    if (typeof o.workspaceId === "string") {
      const { member, id } = this.split(o.workspaceId, "workspace");
      return { member, msg: { ...msg, workspaceId: id } as PhoneMsg };
    }
    const target = this.members.find((m) => m.id === via);
    if (target) return { member: target, msg };
    if (msg.t === "panel.frameDone") return { member: this.frameFrom ?? this.primary, msg };
    if (msg.t === "panes.get") return null;
    // The rest (usage, notifications, models, the Android panel) from the primary.
    return { member: this.primary, msg };
  }

  async request(msg: PhoneMsg, via?: string): Promise<DesktopMsg> {
    const to = this.route(msg, via);
    if (to) return this.lift(to.member, await to.member.client.request(to.msg));
    const live = this.members.filter((m) => m.client.getState().socket === "open");
    for (const m of this.members) if (m.client.getState().socket === "closed") m.client.reconnect();
    if (!live.length) throw new Error("No machine connected");
    return Promise.any(live.map((m) => m.client.request(msg)));
  }

  fire(msg: PhoneMsg) {
    const to = this.route(msg);
    if (to) to.member.client.fire(to.msg);
    else for (const m of this.members) m.client.fire(msg);
  }

  onMessage = (fn: (msg: DesktopMsg) => void) => {
    this.streamListeners.add(fn);
    return () => void this.streamListeners.delete(fn);
  };

  sendMessage(paneId: string, text: string): Promise<DesktopMsg> {
    const { member, id } = this.split(paneId, "pane");
    return member.client.sendMessage(id, text).then((r) => this.lift(member, r));
  }

  reconnect() {
    for (const m of this.members) if (m.client.getState().socket !== "open") m.client.reconnect();
  }

  close() {
    this.unsubs.forEach((u) => u());
    for (const m of this.members) m.client.close();
  }
}

/** The view of one machine's link: the state with that machine's connection, welcome, agents and error. */
export function forMachine(state: ClientState, machine: string | undefined): ClientState {
  const m = machine ? state.machines.find((x) => x.id === machine) : undefined;
  if (!m) return state;
  return { ...state, socket: m.socket, desktopOnline: m.desktopOnline, welcome: m.welcome, providers: m.providers, defaultProvider: m.defaultProvider, error: m.error };
}
