// `?mock=1`: an in-memory desktop so the UI runs without relay or winfer.
import { BaseClient } from "./client";
import type { DesktopMsg, Message, Pane, PhoneMsg, Provider, Workspace } from "./protocol";

const now = Date.now();
const min = 60_000;

const PANES: Pane[] = [
  {
    id: "p-auth",
    workspaceId: "w-winfer",
    title: "claude · auth refactor",
    workspace: "winfer",
    provider: "claude",
    status: "needs-input",
    message: "Allow Bash: pnpm test src/features/auth?",
    updatedAt: now - 1 * min,
    request: { kind: "approval", text: "Bash(pnpm test src/features/auth)\nRun the auth test suite" },
    reply: null,
  },
  {
    id: "p-docs",
    workspaceId: "w-winfer",
    title: "codex · docs",
    workspace: "winfer",
    provider: "codex",
    status: "done",
    message: "Updated README with the mobile pairing section.",
    updatedAt: now - 6 * min,
    request: null,
    reply: "Updated README with the mobile pairing section.",
  },
  {
    id: "p-relay",
    workspaceId: "w-mobile",
    title: "claude · relay worker",
    workspace: "winfer-mobile-app",
    provider: "claude",
    status: "working",
    message: "Running wrangler dev…",
    updatedAt: now - 20_000,
    request: null,
    reply: null,
  },
  {
    id: "p-q",
    workspaceId: "w-api",
    title: "opencode · schema",
    workspace: "api",
    provider: "opencode",
    status: "needs-input",
    message: "Should migrations be reversible?",
    updatedAt: now - 3 * min,
    request: {
      kind: "question",
      text: "Should migrations be reversible, or forward-only?",
      options: [
        { n: 1, label: "Reversible", detail: "Every migration ships an up and a down step" },
        { n: 2, label: "Forward-only", detail: "Simpler; roll back by writing a new migration" },
        { n: 3, label: "Mixed", detail: "Reversible for schema, forward-only for data" },
        { n: 4, label: "Type something." },
      ],
    },
    reply: null,
  },
  {
    id: "p-idle",
    workspaceId: "w-winfer",
    title: "shell",
    workspace: "winfer",
    provider: null,
    status: "idle",
    message: null,
    updatedAt: now - 60 * min,
    request: null,
    reply: null,
  },
];

const WORKSPACES: Workspace[] = [
  { id: "w-winfer", name: "winfer", active: true },
  { id: "w-mobile", name: "winfer-mobile-app", active: false },
  { id: "w-api", name: "api", active: false },
  { id: "w-site", name: "website", active: false },
];

const PROVIDERS: Provider[] = [
  { id: "claude", name: "Claude Code" },
  { id: "codex", name: "Codex" },
  { id: "opencode", name: "OpenCode" },
];

const m = (id: string, role: Message["role"], text: string, ago: number, tool?: string): Message => ({
  id,
  role,
  text,
  tool,
  ts: now - ago * min,
});

const THREADS: Record<string, Message[]> = {
  "p-auth": [
    m("1", "user", "Refactor the auth store to use zustand slices and keep the token refresh logic intact.", 14),
    m("2", "assistant", "I'll start by reading the current store and its callers.", 13),
    m("3", "tool", "Read src/stores/auth.ts", 13, "Read"),
    m("4", "tool", "Grep \"useAuth\" — 14 matches", 13, "Grep"),
    m(
      "5",
      "assistant",
      "The store mixes **session state** and **refresh timers**. Plan:\n\n1. Split into `sessionSlice` and `refreshSlice`\n2. Move `scheduleRefresh()` into the refresh slice\n3. Keep the public `useAuth()` hook unchanged\n\n```ts\nexport const useAuth = create<AuthState>()((...a) => ({\n  ...sessionSlice(...a),\n  ...refreshSlice(...a),\n}));\n```",
      11,
    ),
    m("6", "tool", "Edit src/stores/auth.ts (+48 −31)", 6, "Edit"),
    m("7", "assistant", "Refactor done. I'd like to run the auth tests to confirm nothing regressed.", 1),
  ],
  "p-docs": [
    m("1", "user", "Add a section on pairing the phone app to the README.", 9),
    m("2", "tool", "Edit README.md (+22)", 7, "Edit"),
    m("3", "assistant", "Updated README with the mobile pairing section.", 6),
  ],
  "p-relay": [
    m("1", "user", "Build the relay worker with Durable Objects.", 4),
    m("2", "assistant", "Scaffolding `relay/` with a hibernatable WebSocket Durable Object.", 3),
    { ...m("3", "tool", "Bash pnpm wrangler dev", 2.5, "Bash"), state: "running", background: true },
    { ...m("4", "tool", "Explore the Durable Object docs", 2, "Agent"), state: "running", background: true },
    { ...m("5", "tool", "Read relay/src/index.ts", 1, "Read"), state: "done" },
    { ...m("6", "tool", "Bash pnpm vitest run", 0.2, "Bash"), state: "running" },
  ],
  "p-q": [
    m("1", "user", "Draft the schema for the usage tables.", 5),
    m("2", "assistant", "Draft ready in `db/schema.sql`. Should migrations be reversible, or forward-only?", 3),
  ],
  "p-idle": [],
};

export class MockClient extends BaseClient {
  readonly name = "Mock desktop";
  private panes = PANES.map((p) => ({ ...p }));
  private threads: Record<string, Message[]> = structuredClone(THREADS);
  private n = 100;

  constructor() {
    super();
    setTimeout(() => {
      this.set({ socket: "open", desktopOnline: true });
      this.apply({ t: "welcome", desktop: this.name, version: "mock", vapidPublicKey: "" });
      this.apply(this.mirror());
    }, 150);
    // A live event so the island, FLIP re-order and ring animate in the demo.
    setTimeout(() => {
      this.push("p-relay", { role: "tool", text: "Bash npx wrangler deploy", tool: "Bash" });
      this.update("p-relay", { status: "needs-input", message: "Allow Bash: npx wrangler deploy?", request: { kind: "approval", text: "Bash(npx wrangler deploy)\nDeploy the relay to Cloudflare" } });
    }, 7000);
  }

  private mirror(id?: string): DesktopMsg {
    return { t: "panes", id, panes: this.panes.map((p) => ({ ...p })), workspaces: WORKSPACES, providers: PROVIDERS, defaultProvider: "claude" };
  }

  private update(id: string, patch: Partial<Pane>) {
    const pane = this.panes.find((p) => p.id === id);
    if (!pane) return;
    Object.assign(pane, patch, { updatedAt: Date.now() });
    this.apply({ t: "status", pane: { ...pane } });
  }

  private push(id: string, msg: Omit<Message, "id" | "ts">) {
    (this.threads[id] ??= []).push({ ...msg, id: String(++this.n), ts: Date.now() });
  }

  private finishLater(id: string, reply: string) {
    setTimeout(() => this.push(id, { role: "tool", text: "Bash pnpm test", tool: "Bash", state: "running" }), 500);
    setTimeout(() => {
      const t = [...(this.threads[id] ?? [])].reverse().find((x) => x.role === "tool" && x.state === "running" && !x.background);
      if (t) t.state = "done";
    }, 1800);
    setTimeout(() => {
      this.push(id, { role: "assistant", text: reply });
      this.update(id, { status: "done", message: reply, reply, request: null });
    }, 2200);
  }

  async request(msg: PhoneMsg): Promise<DesktopMsg> {
    await new Promise((r) => setTimeout(r, 120));
    const id = String(++this.n);
    switch (msg.t) {
      case "hello":
      case "panes.get":
        this.apply(this.mirror());
        return this.mirror(id);
      case "pane.new": {
        const ws = WORKSPACES.find((w) => w.id === msg.workspaceId);
        const provider = PROVIDERS.find((p) => p.id === msg.provider) ?? PROVIDERS[0];
        if (!ws) return { t: "ack", id, ok: false, error: "That workspace is gone." };
        const paneId = `p-new-${++this.n}`;
        const text = msg.text?.trim();
        this.panes.push({ id: paneId, workspaceId: ws.id, workspace: ws.name, title: text ? text.slice(0, 40) : ws.name, provider: provider.id, status: text ? "working" : "idle", message: null, updatedAt: Date.now(), request: null, reply: null });
        this.threads[paneId] = [];
        this.apply(this.mirror());
        if (text) {
          this.push(paneId, { role: "user", text });
          this.finishLater(paneId, `Started on "${text.slice(0, 40)}". Done.`);
        }
        return { t: "ack", id, ok: true, paneId };
      }
      case "thread.get": {
        const reply: DesktopMsg = {
          t: "thread",
          id,
          paneId: msg.paneId,
          messages: [...(this.threads[msg.paneId] ?? [])],
          source: msg.paneId === "p-idle" ? "screen" : "claude",
        };
        this.apply(reply);
        return reply;
      }
      case "send":
        this.push(msg.paneId, { role: "user", text: msg.text });
        this.update(msg.paneId, { status: "working", message: null, request: null });
        this.finishLater(msg.paneId, `Got it — "${msg.text.slice(0, 40)}". Done.`);
        return { t: "ack", id, ok: true };
      case "approve":
        this.update(msg.paneId, { status: "working", message: "Running…", request: null });
        this.finishLater(msg.paneId, "All 42 auth tests pass. The refactor is complete.");
        return { t: "ack", id, ok: true };
      case "decline":
        this.update(msg.paneId, { status: "needs-input", message: "Declined. What should I do instead?", request: null });
        return { t: "ack", id, ok: true };
      case "interrupt":
        this.update(msg.paneId, { status: "idle", message: "Interrupted", request: null });
        return { t: "ack", id, ok: true };
      case "choose": {
        const pane = this.panes.find((p) => p.id === msg.paneId);
        const o = pane?.request?.options?.find((x) => x.n === msg.option);
        if (!o) return { t: "ack", id, ok: false, error: "That choice is no longer on screen." };
        const answer = msg.text ?? o.label;
        this.push(msg.paneId, { role: "user", text: answer });
        this.update(msg.paneId, { status: "working", message: `Going with ${answer.toLowerCase()}…`, request: null });
        this.finishLater(msg.paneId, `Done — migrations are now **${answer.toLowerCase()}**.`);
        return { t: "ack", id, ok: true };
      }
      case "push.subscribe":
      case "push.unsubscribe":
        return { t: "ack", id, ok: true };
    }
  }

  reconnect() {}
  close() {}
}
