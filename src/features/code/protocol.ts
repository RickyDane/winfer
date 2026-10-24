/**
 * The web UI's view of winfer code's backend protocol: mirrors `harness::event::{Ev, Cmd}` in
 * their JSON shape (see `event_json` in src-tauri/src/harness/mod.rs), so a real transport can
 * stream the same events the TUI consumes.
 */

export type Mode = "ask" | "auto-edit" | "yolo" | "plan" | "yolo-plan";
export type Decision = "allow" | "always" | "deny";

export interface Question {
  header: string;
  question: string;
  options: { label: string; description: string }[];
  multi: boolean;
}

export interface Approval {
  id: string;
  tool: string;
  title: string;
  detail: string;
  diff?: string | null;
  questions: Question[];
}

export interface Todo {
  content: string;
  status: "pending" | "in_progress" | "completed";
}

export interface TaskEv {
  id: string;
  tool_id?: string | null;
  title?: string | null;
  description?: string | null;
  agent?: string | null;
  tools?: number | null;
  last?: string | null;
  status?: "running" | "done" | "failed" | "stopped" | null;
  /** File receiving a background shell's output (read with `task_log`). */
  output_file?: string | null;
  exit_code?: number | null;
}

export interface Usage {
  context: number;
  window: number;
  input: number;
  cached: number;
  output: number;
  cost?: number | null;
  /** Output tokens of the running (or last) turn. */
  turn?: number;
  /** Input tokens of the running (or last) turn, summed over its requests, and the cached part. */
  turn_input?: number;
  turn_cached?: number;
  turn_streamed?: number;
}

export type Ev =
  | { type: "session"; id: string }
  | { type: "model"; model: string }
  | { type: "mode"; mode: Mode }
  | { type: "text_delta"; id: string; text: string }
  | { type: "text"; id: string; text: string }
  | { type: "reasoning_delta"; id: string; text: string }
  | { type: "tool_start"; id: string; name: string; title: string; input: unknown }
  | { type: "tool_output"; id: string; chunk: string }
  | { type: "tool_end"; id: string; ok: boolean; output: string; diff?: string | null }
  | ({ type: "approval" } & Approval)
  | ({ type: "task" } & TaskEv)
  | { type: "todos"; todos: Todo[] }
  | { type: "skills"; skills: string[] }
  | ({ type: "usage" } & Usage)
  | { type: "notice"; text: string }
  | { type: "error"; text: string }
  | { type: "turn_start" }
  | { type: "turn_done" }
  | { type: "exited"; text: string }
  | { type: "undone"; files?: string[] | null; error?: string | null }
  | { type: "bg_note"; text: string }
  // `winfer code --serve` only:
  | {
      type: "ready";
      session: string;
      backend_session?: string | null;
      title?: string;
      model: string;
      mode: Mode;
      effort?: string | null;
      cwd: string;
      branch?: string | null;
      trim_at?: number;
      compact_at?: number;
      mcp?: string[];
    }
  | { type: "sessions"; sessions: SessionInfo[] }
  | { type: "btw"; id: string; answer?: string; error?: string }
  | { type: "task_log"; id: string; text: string | null; file?: string | null }
  | { type: "models"; models: ModelInfo[] }
  | { type: "restore"; transcript: { role: string; text: string; state?: string | null }[] }
  | { type: "user"; text: string }
  | { type: "cleared" }
  /** An approval answered outside the web UI (the app's approve button, its broadcast bar). */
  | { type: "answered"; id: string };

/** A pasted or dropped image: base64 without the `data:` prefix. */
export interface ImageData {
  mime: string;
  data: string;
}

export interface SessionInfo {
  id: string;
  title: string;
  model: string;
  /** Unix seconds. */
  updated: number;
  messages: number;
}

export interface ModelInfo {
  /** `backend/model`, e.g. `claude/opus`. */
  spec: string;
  label: string;
  detail: string;
}

export type Cmd =
  | { type: "send"; text: string; images?: ImageData[] }
  | { type: "steer"; text: string; images?: ImageData[] }
  | { type: "interrupt" }
  | { type: "approve"; id: string; decision: Decision }
  | { type: "answer"; id: string; answers: string[] }
  | { type: "set_model"; model: string }
  | { type: "set_effort"; effort: string | null }
  | { type: "set_mode"; mode: Mode }
  | { type: "compact" }
  | { type: "undo"; files: boolean }
  | { type: "stop_task"; id: string }
  | { type: "new" }
  | { type: "set_compaction"; trim?: number; compact?: number }
  | { type: "diff" }
  | { type: "btw"; id: string; question: string }
  | { type: "export"; markdown: string }
  | { type: "sessions" }
  | { type: "task_log"; id: string }
  | { type: "resume"; id: string };

/** Where events come from and commands go: the mock below, or a socket to `winfer code --serve`. */
export interface Transport {
  connect(onEvent: (ev: Ev) => void): () => void;
  send(cmd: Cmd): void;
}

export const MODES: { id: Mode; label: string; hint: string }[] = [
  { id: "ask", label: "Ask", hint: "Ask before edits, commands and network" },
  { id: "auto-edit", label: "Auto-edit", hint: "File edits go through; commands still ask" },
  { id: "plan", label: "Plan", hint: "Read-only: plan without changing anything" },
  { id: "yolo-plan", label: "Yolo plan", hint: "Plan; tools run without approval" },
  { id: "yolo", label: "Yolo", hint: "Nothing asks" },
];

/** Mirrors `tui::app::COMMANDS`. */
export const COMMANDS: [string, string][] = [
  ["/model", "Switch model (ChatGPT, Claude, Codex)"],
  ["/effort", "Set reasoning effort"],
  ["/mode", "Permission mode: ask, auto-edit, plan, yolo-plan, yolo"],
  ["/new", "Start a new conversation"],
  ["/resume", "Resume a past session"],
  ["/btw", "Ask a side question about this session without interrupting the turn"],
  ["/compact", "Summarize the conversation to free context"],
  ["/compaction", "When to trim and auto-compact (% of context)"],
  ["/undo", "Undo the last turn: its messages and file changes"],
  ["/init", "Create an AGENTS.md for this project"],
  ["/review", "Review uncommitted changes"],
  ["/diff", "Show uncommitted git changes"],
  ["/cost", "Token usage, context breakdown and cost"],
  ["/copy", "Copy the last reply"],
  ["/export", "Save the conversation as Markdown"],
  ["/agents", "Subagents and background tasks (ctrl+g)"],
  ["/tasks", "Background shells and their output (ctrl+b)"],
  ["/mcp", "MCP servers in use"],
  ["/status", "Session, model and directory"],
  ["/sidebar", "Plan in a sidebar on wide panes (on/off)"],
  ["/login", "How to sign in to each backend"],
  ["/help", "Keys and commands"],
];

export const INIT_PROMPT =
  "Analyze this codebase and create an AGENTS.md file at the repository root with concise guidance for coding agents: what the project is, how it's laid out, how to build, run, test and lint it, code style and conventions, and anything non-obvious. If one exists, improve it instead.";
export const REVIEW_PROMPT =
  "Review the uncommitted changes in this repository (git diff, including staged). Look for bugs, regressions, missing edge cases and unclear code. Report findings ranked by severity with file:line references. Don't change any files.";

/** The scripted session's models (a real session lists the backends' own). */
export const MOCK_MODELS: ModelInfo[] = [
  { spec: "claude/opus", label: "Claude Opus", detail: "Claude Code" },
  { spec: "claude/sonnet", label: "Claude Sonnet", detail: "Claude Code" },
  { spec: "claude/haiku", label: "Claude Haiku", detail: "Claude Code · fastest" },
  { spec: "chatgpt/gpt-6-luna", label: "GPT-6 Luna", detail: "ChatGPT account" },
  { spec: "chatgpt/gpt-6.1-sol", label: "GPT-6.1 Sol", detail: "ChatGPT account" },
  { spec: "codex/gpt-6.1-sol", label: "GPT-6.1-Sol", detail: "Codex · Latest workhorse model" },
  { spec: "cursor/sonnet-4.5", label: "Sonnet 4.5", detail: "Cursor" },
  { spec: "opencode/anthropic/claude-sonnet-4-5", label: "anthropic/claude-sonnet-4-5", detail: "OpenCode · anthropic" },
  { spec: "pi/openai/gpt-6.1-sol", label: "openai/gpt-6.1-sol", detail: "Pi · openai" },
  { spec: "codex/gpt-6.1-sol-fast", label: "GPT-6.1-Sol (Fast)", detail: "Codex · 2x speed, increased usage" },
];
