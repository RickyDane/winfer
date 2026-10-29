import type { DesktopMsg, Message, Pane, PhoneMsg, Provider, ThreadSource, Workspace } from "./protocol";

export interface Thread {
  messages: Message[];
  source: ThreadSource;
  loadedAt: number;
}

export interface ClientState {
  socket: "connecting" | "open" | "closed";
  /** null until the relay reports presence. */
  desktopOnline: boolean | null;
  welcome: { desktop: string; version: string; vapidPublicKey: string } | null;
  panes: Pane[];
  /** The desktop's workspaces (empty from desktops that don't send them). */
  workspaces: Workspace[];
  /** Agents the phone can start sessions with. */
  providers: Provider[];
  defaultProvider: string | null;
  threads: Record<string, Thread>;
  error: string | null;
}

export interface DesktopClient {
  readonly name: string;
  getState(): ClientState;
  subscribe(fn: () => void): () => void;
  /** Send a request and wait for the reply carrying the same id. */
  request(msg: PhoneMsg): Promise<DesktopMsg>;
  /** Show the user's message immediately, then reconcile it with the desktop transcript. */
  sendMessage(paneId: string, text: string): Promise<DesktopMsg>;
  reconnect(): void;
  close(): void;
}

export const initialState = (): ClientState => ({
  socket: "connecting",
  desktopOnline: null,
  welcome: null,
  panes: [],
  workspaces: [],
  providers: [],
  defaultProvider: null,
  threads: {},
  error: null,
});

/** Shared state + reducer for real and mock clients. */
export abstract class BaseClient implements DesktopClient {
  protected state = initialState();
  private optimistic = new Map<string, { paneId: string; message: Message; known: Set<string> }>();
  private messageSeq = 0;
  private listeners = new Set<() => void>();
  abstract readonly name: string;
  abstract request(msg: PhoneMsg): Promise<DesktopMsg>;
  abstract reconnect(): void;
  abstract close(): void;

  getState = () => this.state;

  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => void this.listeners.delete(fn);
  };

  protected set(patch: Partial<ClientState>) {
    this.state = { ...this.state, ...patch };
    for (const fn of this.listeners) fn();
  }

  async sendMessage(paneId: string, text: string): Promise<DesktopMsg> {
    const id = `local-${Date.now()}-${++this.messageSeq}`;
    const thread = this.state.threads[paneId];
    const message: Message = { id, role: "user", text, ts: Date.now() };
    this.optimistic.set(id, { paneId, message, known: new Set(thread?.messages.map((m) => m.id)) });
    this.set({ threads: { ...this.state.threads, [paneId]: { source: thread?.source ?? "screen", loadedAt: thread?.loadedAt ?? 0, messages: [...(thread?.messages ?? []), message] } } });
    try {
      const reply = await this.request({ t: "send", paneId, text });
      if (reply.t === "ack" && !reply.ok) throw new Error(reply.error || "Desktop refused");
      void this.request({ t: "thread.get", paneId, limit: 200 }).catch(() => {});
      return reply;
    } catch (error) {
      this.optimistic.delete(id);
      const current = this.state.threads[paneId];
      if (current) this.set({ threads: { ...this.state.threads, [paneId]: { ...current, messages: current.messages.filter((m) => m.id !== id) } } });
      throw error;
    }
  }

  /** Fold a desktop message into state. */
  protected apply(msg: DesktopMsg) {
    switch (msg.t) {
      case "welcome":
        this.set({ welcome: { desktop: msg.desktop, version: msg.version, vapidPublicKey: msg.vapidPublicKey }, error: null });
        break;
      case "panes":
        this.set({
          panes: msg.panes,
          ...(msg.workspaces && { workspaces: msg.workspaces }),
          ...(msg.providers && { providers: msg.providers }),
          ...(msg.defaultProvider !== undefined && { defaultProvider: msg.defaultProvider }),
        });
        break;
      case "status": {
        // In place, so the desktop's order holds; a pane we didn't know goes last.
        const i = this.state.panes.findIndex((p) => p.id === msg.pane.id);
        const panes = [...this.state.panes];
        if (i < 0) panes.push(msg.pane);
        else panes[i] = msg.pane;
        this.set({ panes });
        break;
      }
      case "thread": {
        const remaining = [...msg.messages];
        const claimed = new Set<string>();
        for (const [id, pending] of this.optimistic) {
          if (pending.paneId !== msg.paneId) continue;
          const echo = msg.messages.find((m) => m.role === "user" && m.text === pending.message.text && !pending.known.has(m.id) && !claimed.has(m.id));
          if (echo) {
            claimed.add(echo.id);
            this.optimistic.delete(id);
            // A repeated identical send must wait for its own echo, not reuse this one.
            for (const next of this.optimistic.values()) if (next.paneId === msg.paneId) next.known.add(echo.id);
          } else remaining.push(pending.message);
        }
        this.set({
          threads: { ...this.state.threads, [msg.paneId]: { messages: remaining, source: msg.source, loadedAt: Date.now() } },
        });
        break;
      }
      case "error":
        if (!msg.id) this.set({ error: msg.error });
        break;
      case "ack":
        break;
    }
  }
}
