import { createContext, useContext } from "react";
import { createStore, useStore, type StoreApi } from "zustand";
import type { Approval, Cmd, Ev, Mode, ModelInfo, SessionInfo, TaskEv, Todo, Transport, Usage } from "./protocol";

export interface Attachment {
  id: string;
  /** `data:` URL, for the thumbnail. */
  url: string;
  mime: string;
  name: string;
}

export type Sheet = "tasklog" | "usage" | "status" | "help" | "mcp" | "login" | "agents" | "sessions" | "btw" | "compaction" | "model" | "effort" | "mode";

export interface Btw {
  id: string;
  question: string;
  answer?: string;
  error?: string;
}

export type Item =
  | { kind: "user"; key: string; text: string; at: number; images?: string[] }
  | { kind: "text"; key: string; text: string; streaming: boolean }
  | { kind: "reasoning"; key: string; text: string; streaming: boolean; started: number; ended?: number }
  | {
      kind: "tool";
      key: string;
      name: string;
      title: string;
      input: unknown;
      output: string;
      ok?: boolean;
      diff?: string | null;
      started: number;
      ended?: number;
    }
  | { kind: "notice"; key: string; text: string; tone: "info" | "error" }
  /** Total time the agent worked since the last user message, shown under its last response. */
  | { kind: "worked"; key: string; ms: number };

export interface CodeState {
  session?: string;
  model: string;
  /** The model actually answering, when the backend resolved an alias. */
  actualModel: string | null;
  models: ModelInfo[];
  cwd?: string;
  backendSession?: string | null;
  title?: string;
  branch?: string | null;
  trimAt: number;
  compactAt: number;
  mcp: string[];
  sessions: SessionInfo[] | null;
  btw: Btw[];
  /** The panel open over the transcript. */
  sheet: Sheet | null;
  /** Tokens streamed in the running turn when it started / now, and when, for tok/s. */
  rate: { tokens: number; at: number; start: number; startAt: number } | null;
  setSheet(sheet: Sheet | null): void;
  /** Images open in the full-size preview, and which one shows. */
  preview: { images: string[]; index: number } | null;
  setPreview(preview: { images: string[]; index: number } | null): void;
  /** The background task whose output is open, and the latest tail of it. */
  logTask: string | null;
  taskLog: { id: string; text: string | null; file?: string | null } | null;
  openTaskLog(id: string): void;
  mode: Mode;
  effort: string;
  items: Item[];
  approvals: Approval[];
  tasks: Record<string, TaskEv>;
  todos: Todo[];
  skills: string[];
  usage: Usage;
  busy: boolean;
  turnStarted?: number;
  exited?: string;
  transport?: Transport;
  apply(ev: Ev): void;
  send(cmd: Cmd): void;
  submit(text: string, images?: Attachment[]): void;
  /** The composer's unsent text and images: kept here so they survive the pane remounting. */
  draft: string;
  attachments: Attachment[];
  setDraft(draft: string): void;
  setAttachments(fn: (a: Attachment[]) => Attachment[]): void;
  attach(t: Transport): () => void;
}

import { isSpec, rememberResolved } from "./names";

let seq = 0;
function findLastIndex<T>(a: T[], f: (x: T) => boolean) {
  for (let i = a.length - 1; i >= 0; i--) if (f(a[i])) return i;
  return -1;
}
const key = () => `k${++seq}`;

/** One store per winfer code pane: its transcript and session state. */
export const createCodeStore = () =>
  createStore<CodeState>((set, get) => ({
  model: "",
  actualModel: null,
  models: [],
  trimAt: 0,
  compactAt: 0,
  mcp: [],
  sessions: null,
  btw: [],
  sheet: null,
  rate: null,
  setSheet: (sheet) => set({ sheet }),
  preview: null,
  setPreview: (preview) => set({ preview }),
  logTask: null,
  taskLog: null,
  openTaskLog: (id) => {
    set({ logTask: id, taskLog: null, sheet: "tasklog" });
    get().send({ type: "task_log", id });
  },
  mode: "ask",
  effort: "high",
  items: [],
  approvals: [],
  tasks: {},
  todos: [],
  skills: [],
  usage: { context: 0, window: 200_000, input: 0, cached: 0, output: 0 },
  busy: false,

  attach(t) {
    set({ transport: t });
    return t.connect((ev) => get().apply(ev));
  },

  send(cmd) {
    if (cmd.type === "set_mode") set({ mode: cmd.mode });
    if (cmd.type === "set_model") set({ model: cmd.model, actualModel: null });
    if (cmd.type === "set_effort") set({ effort: cmd.effort ?? "default" });
    if (cmd.type === "undo" || cmd.type === "new") set({ approvals: [] });
    if (cmd.type === "btw") set((s) => ({ btw: [...s.btw, { id: cmd.id, question: cmd.question }], sheet: "btw" }));
    if (cmd.type === "sessions") set({ sessions: null });
    if (cmd.type === "set_compaction") set((s) => ({ trimAt: cmd.trim ?? s.trimAt, compactAt: cmd.compact ?? s.compactAt }));
    if (cmd.type === "approve" || cmd.type === "answer")
      set((s) => ({ approvals: s.approvals.filter((a) => a.id !== cmd.id) }));
    get().transport?.send(cmd);
  },

  draft: "",
  attachments: [],
  setDraft: (draft) => set({ draft }),
  setAttachments: (fn) => set((s) => ({ attachments: fn(s.attachments) })),

  submit(text, attachments = []) {
    const t = text.trim();
    if (!t && !attachments.length) return;
    const urls = attachments.map((a) => a.url);
    set((s) => ({ items: [...s.items, { kind: "user", key: key(), text: t, at: Date.now(), images: urls.length ? urls : undefined }] }));
    const images = attachments.map((a) => ({ mime: a.mime, data: a.url.slice(a.url.indexOf(",") + 1) }));
    get().send({ type: get().busy ? "steer" : "send", text: t, ...(images.length ? { images } : {}) });
  },

  apply(ev) {
    set((s) => {
      const items = s.items;
      const findLast = (k: Item["kind"], id: string) =>
        findLastIndex(items, (i) => i.kind === k && i.key === id);
      const replace = (idx: number, next: Item) => {
        const copy = items.slice();
        copy[idx] = next;
        return copy;
      };
      // Reasoning ends once anything else starts streaming.
      const closeReasoning = (list: Item[]) =>
        list.map((i) => (i.kind === "reasoning" && i.streaming ? { ...i, streaming: false, ended: Date.now() } : i));

      switch (ev.type) {
        case "session":
          return { backendSession: ev.id };
        case "model":
          // `--serve` reports a switch as the `backend/model` spec; backends report the model
          // actually answering (an alias resolved: claude/opus → claude-opus-5-5).
          if (isSpec(ev.model)) return { model: ev.model, actualModel: null };
          if (s.model) rememberResolved(s.model, ev.model);
          return { actualModel: ev.model };
        case "mode":
          return { mode: ev.mode };
        case "text_delta": {
          const i = findLast("text", ev.id);
          if (i >= 0) {
            const it = items[i] as Extract<Item, { kind: "text" }>;
            return { items: replace(i, { ...it, text: it.text + ev.text }) };
          }
          return { items: [...closeReasoning(items), { kind: "text", key: ev.id, text: ev.text, streaming: true }] };
        }
        case "text": {
          const i = findLast("text", ev.id);
          if (i >= 0) return { items: replace(i, { kind: "text", key: ev.id, text: ev.text, streaming: false }) };
          return { items: [...closeReasoning(items), { kind: "text", key: ev.id, text: ev.text, streaming: false }] };
        }
        case "reasoning_delta": {
          const i = findLast("reasoning", ev.id);
          if (i >= 0) {
            const it = items[i] as Extract<Item, { kind: "reasoning" }>;
            return { items: replace(i, { ...it, text: it.text + ev.text }) };
          }
          return {
            items: [...items, { kind: "reasoning", key: ev.id, text: ev.text, streaming: true, started: Date.now() }],
          };
        }
        case "tool_start":
          return {
            items: [
              ...closeReasoning(items),
              { kind: "tool", key: ev.id, name: ev.name, title: ev.title, input: ev.input, output: "", started: Date.now() },
            ],
          };
        case "tool_output": {
          const i = findLast("tool", ev.id);
          if (i < 0) return {};
          const it = items[i] as Extract<Item, { kind: "tool" }>;
          return { items: replace(i, { ...it, output: it.output + ev.chunk }) };
        }
        case "tool_end": {
          const i = findLast("tool", ev.id);
          if (i < 0) return {};
          const it = items[i] as Extract<Item, { kind: "tool" }>;
          return {
            items: replace(i, { ...it, ok: ev.ok, output: ev.output || it.output, diff: ev.diff, ended: Date.now() }),
          };
        }
        case "approval": {
          const { type: _t, ...a } = ev;
          return { approvals: [...s.approvals, a] };
        }
        case "task": {
          const { type: _t, ...t } = ev;
          const prev = s.tasks[t.id] ?? { id: t.id };
          const merged = { ...prev };
          for (const [k, v] of Object.entries(t)) if (v != null) (merged as Record<string, unknown>)[k] = v;
          return { tasks: { ...s.tasks, [t.id]: merged } };
        }
        case "todos":
          return { todos: ev.todos };
        case "skills":
          return { skills: ev.skills };
        case "usage": {
          const { type: _t, ...u } = ev;
          const streamed = u.turn_streamed ?? u.output;
          const now = Date.now();
          const rate = s.busy ? (s.rate ? { ...s.rate, tokens: streamed, at: now } : { start: streamed, startAt: now, tokens: streamed, at: now }) : s.rate;
          return { usage: { ...s.usage, ...u }, rate };
        }
        case "notice":
        case "bg_note":
          return { items: [...items, { kind: "notice", key: key(), text: ev.text, tone: "info" }] };
        case "error":
          return { items: [...items, { kind: "notice", key: key(), text: ev.text, tone: "error" }] };
        case "turn_start":
          return { busy: true, turnStarted: Date.now(), rate: null, usage: { ...s.usage, turn: 0, turn_input: 0, turn_cached: 0, turn_streamed: 0 } };
        case "turn_done": {
          const closed = closeReasoning(items).map((i) => (i.kind === "text" && i.streaming ? { ...i, streaming: false } : i));
          // Follow-up turns without a new user message (queued steers, background
          // task wake-ups) add up into one total under the latest response.
          const lastUser = findLastIndex(closed, (i) => i.kind === "user");
          let ms = s.turnStarted ? Date.now() - s.turnStarted : 0;
          const kept = closed.filter((i, idx) => {
            if (i.kind !== "worked" || idx < lastUser) return true;
            ms += i.ms;
            return false;
          });
          const worked = kept.slice(lastUser + 1).some((i) => i.kind === "text" || i.kind === "tool" || i.kind === "reasoning");
          return {
            busy: false,
            approvals: [],
            items: worked && ms > 0 ? [...kept, { kind: "worked", key: key(), ms }] : kept,
          };
        }
        case "exited":
          return { busy: false, exited: ev.text };
        case "ready":
          return {
            session: ev.session,
            backendSession: ev.backend_session,
            title: ev.title,
            model: ev.model,
            actualModel: ev.model === s.model ? s.actualModel : null,
            mode: ev.mode,
            effort: ev.effort ?? "default",
            cwd: ev.cwd,
            branch: ev.branch,
            trimAt: ev.trim_at ?? s.trimAt,
            compactAt: ev.compact_at ?? s.compactAt,
            mcp: ev.mcp ?? s.mcp,
            exited: undefined,
          };
        case "sessions":
          return { sessions: ev.sessions };
        case "task_log":
          return ev.id === s.logTask ? { taskLog: { id: ev.id, text: ev.text, file: ev.file } } : {};
        case "btw":
          return { btw: s.btw.map((b) => (b.id === ev.id ? { ...b, answer: ev.answer, error: ev.error } : b)) };
        case "models":
          return { models: ev.models };
        case "user":
          return { items: [...items, { kind: "user", key: key(), text: ev.text, at: Date.now() }] };
        case "answered":
          return { approvals: s.approvals.filter((a) => a.id !== ev.id) };
        case "cleared":
          return { items: [], approvals: [], tasks: {}, todos: [], busy: false, usage: { ...s.usage, context: 0, input: 0, cached: 0, output: 0, cost: null, turn: 0 }, rate: null };
        case "restore": {
          const restored: Item[] = ev.transcript.map((e): Item => {
            if (e.role === "user") return { kind: "user", key: key(), text: e.text, at: 0 };
            if (e.role === "assistant") return { kind: "text", key: key(), text: e.text, streaming: false };
            if (e.role === "tool") {
              const [name, title = ""] = e.text.split("\t");
              return { kind: "tool", key: key(), name, title, input: null, output: "", ok: e.state !== "failed", started: 0, ended: 0 };
            }
            return { kind: "notice", key: key(), text: e.text, tone: "info" };
          });
          return { items: [...restored, { kind: "notice", key: key(), text: "Resumed session", tone: "info" }] };
        }
        case "undone": {
          const lastUser = findLastIndex(items, (i) => i.kind === "user");
          const kept = lastUser >= 0 ? items.slice(0, lastUser) : items;
          const text = ev.error ? `Undo failed: ${ev.error}` : `Undid last turn${ev.files?.length ? ` · reverted ${ev.files.join(", ")}` : ""}`;
          return { items: [...kept, { kind: "notice", key: key(), text, tone: ev.error ? "error" : "info" }] };
        }
      }
    });
  },
}));

export const CodeStoreContext = createContext<StoreApi<CodeState> | null>(null);

export function useCode(): CodeState;
export function useCode<T>(sel: (s: CodeState) => T): T;
export function useCode<T>(sel?: (s: CodeState) => T) {
  const store = useContext(CodeStoreContext);
  if (!store) throw new Error("useCode outside a winfer code pane");
  return useStore(store, sel ?? ((s) => s as unknown as T));
}
