import { applyTheme } from "./theme";
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
  welcome: { desktop: string; version: string; vapidPublicKey: string; server?: boolean } | null;
  panes: Pane[];
  /** The desktop's workspaces (empty from desktops that don't send them). */
  workspaces: Workspace[];
  /** Agents the phone can start sessions with. */
  providers: Provider[];
  defaultProvider: string | null;
  /** Desktop's `codeWeb.compactTools` setting (defaults to true, matching native-gpui). */
  compactTools: boolean;
  /** Desktop's `codeWeb.showBgNotes` setting (defaults to true). */
  showBgNotes: boolean;
  threads: Record<string, Thread>;
  error: string | null;
  /** With several paired machines: each one's own link and agents (empty with one). */
  machines: Machine[];
}

/** One paired machine, as merged into the phone's combined view. */
export interface Machine {
  id: string;
  name: string;
  /** The one used for notifications, usage, and the theme. */
  primary: boolean;
  socket: ClientState["socket"];
  desktopOnline: boolean | null;
  welcome: ClientState["welcome"];
  providers: Provider[];
  defaultProvider: string | null;
  error: string | null;
}

export interface DesktopClient {
  readonly name: string;
  getState(): ClientState;
  subscribe(fn: () => void): () => void;
  /** Send a request and wait for the reply carrying the same id. */
  /** `via`: the machine to ask, for requests that don't name a pane or workspace. */
  request(msg: PhoneMsg, via?: string): Promise<DesktopMsg>;
  /** Send without waiting for (or getting) a reply: input events, frame acks. */
  fire(msg: PhoneMsg): void;
  /** Every message from the desktop as it comes (streams: terminal output, panel frames). */
  onMessage(fn: (msg: DesktopMsg) => void): () => void;
  /** Show the user's message immediately, then reconcile it with the desktop transcript. */
  sendMessage(paneId: string, text: string): Promise<DesktopMsg>;
  reconnect(): void;
  close(): void;
  /** A bare pane id (from a notification) in this client's terms, when it names them differently. */
  canonicalId?(id: string): string;
}

export const initialState = (): ClientState => ({
  socket: "connecting",
  desktopOnline: null,
  welcome: null,
  panes: [],
  workspaces: [],
  providers: [],
  defaultProvider: null,
  compactTools: true,
  showBgNotes: true,
  threads: {},
  error: null,
  machines: [],
});

/** Shared state + reducer for real and mock clients. */
export abstract class BaseClient implements DesktopClient {
  protected state = initialState();
  private optimistic = new Map<string, { paneId: string; message: Message; known: Set<string> }>();
  private messageSeq = 0;
  private listeners = new Set<() => void>();
  private streams = new Set<(msg: DesktopMsg) => void>();
  abstract readonly name: string;
  /** Follow the desktop's theme (off for all but the primary of several machines). */
  themed = true;
  abstract request(msg: PhoneMsg): Promise<DesktopMsg>;
  abstract fire(msg: PhoneMsg): void;

  onMessage = (fn: (msg: DesktopMsg) => void) => {
    this.streams.add(fn);
    return () => void this.streams.delete(fn);
  };
  abstract reconnect(): void;
  abstract close(): void;

  getState = () => this.state;

  /** The welcome said whether this is a server. */
  protected onServer(_server: boolean) {}

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
    for (const fn of this.streams) fn(msg);
    switch (msg.t) {
      case "welcome":
        this.set({ welcome: { desktop: msg.desktop, version: msg.version, vapidPublicKey: msg.vapidPublicKey, server: msg.server }, error: null });
        if (msg.server !== undefined) this.onServer(msg.server);
        break;
      case "panes":
        if (msg.theme && this.themed) applyTheme(msg.theme);
        this.set({
          panes: msg.panes,
          ...(msg.workspaces && { workspaces: msg.workspaces }),
          ...(msg.providers && { providers: msg.providers }),
          ...(msg.defaultProvider !== undefined && { defaultProvider: msg.defaultProvider }),
          ...(msg.compactTools !== undefined && { compactTools: msg.compactTools }),
          ...(msg.showBgNotes !== undefined && { showBgNotes: msg.showBgNotes }),
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
      case "config":
      case "models":
        break;
    }
  }
}
