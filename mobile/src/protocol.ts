// Types from docs/mobile/protocol.md (v1). Keep in sync with the spec.
export type AgentStatus = "idle" | "working" | "needs-input" | "done" | "error";

export interface RequestOption {
  /** Typing this number into the pane picks the option. */
  n: number;
  label: string;
  detail?: string;
}

export interface PaneRequest {
  kind: "approval" | "question";
  text: string;
  /** Numbered choices shown under the question, when the agent offers them. */
  options?: RequestOption[];
}

/** "Type something." / "Other" rows open a text field instead of answering. */
export const isFreeText = (o: RequestOption) => /^(type something|something else|other|custom|write (your own|my own))\b/i.test(o.label.trim());

/** A winfer code agent's model and reasoning effort (other agents don't send it). */
export interface PaneConfig {
  /** The model's spec, as `ModelOption.id`. */
  model: string;
  /** The model's display name. */
  label: string;
  effort: string;
  /** The levels the current model takes, "default" first. */
  efforts: string[];
  /** The chat's permission mode ("Yolo", "Ask"…), and whether it runs without asking. */
  mode?: string;
  yolo?: boolean;
  cwd?: string | null;
  branch?: string | null;
}

/** The desktop's color theme (winfer's themes.json entry), so the phone matches it. */
export interface Theme {
  id: string;
  mode: "dark" | "light";
  ui: Record<string, string>;
}

export interface ModelOption {
  id: string;
  name: string;
  detail?: string;
  efforts: string[];
}

export interface Pane {
  id: string;
  title: string;
  workspace: string;
  /** Older desktops don't send these. */
  workspaceId?: string;
  /** Put aside in the desktop's archive (still running). */
  archived?: boolean;
  provider: string | null;
  status: AgentStatus;
  message: string | null;
  updatedAt: number;
  request: PaneRequest | null;
  reply: string | null;
  config?: PaneConfig | null;
}

/** A `visualize` report, normalised by the desktop (`viz::Report::to_json`). */
export interface Report {
  title: string | null;
  subtitle: string | null;
  blocks: ReportBlock[];
}

export interface Series {
  name: string;
  values: number[];
}

export type ReportBlock = { title: string | null; caption: string | null; span: number | "full" | null } & (
  | { type: "stats"; items: { label: string; value: string; delta: string | null; note: string | null }[] }
  | { type: "bar"; labels: string[]; series: Series[]; horizontal: boolean; stacked: boolean; unit: string | null }
  | { type: "line"; labels: string[]; series: Series[]; area: boolean; unit: string | null }
  | { type: "pie"; slices: { label: string; value: number }[]; ring: boolean; center: { value: string; label: string } | null; unit: string | null }
  | { type: "table"; columns: { label: string; align: "left" | "right" | "center" }[]; rows: string[][] }
  | { type: "progress"; items: { label: string; value: number; max: number }[] }
  | { type: "text"; markdown: string }
);

export interface Message {
  id: string;
  /** "thinking": a text-less marker ending the thread while the agent reasons; never shown. */
  role: "user" | "assistant" | "tool" | "thinking" | "bg";
  text: string;
  tool?: string;
  /** Short human-readable tool description (e.g. Bash/Agent `description`), used in compact rows. */
  desc?: string | null;
  ts: number | null;
  /** Tool completion timestamp (ms epoch), when known. */
  ended?: number | null;
  /** Tools: whether the call is still running, when the session says. */
  state?: "running" | "done" | "failed";
  /** Tools: a background shell or subagent, which keeps running after its call returns. */
  background?: boolean;
  /** Tools: the chart, table or tiles a `visualize` call made; shown instead of the tool row. */
  report?: Report;
  /** "bg": a finished background task: `text` is its one-line summary, `detail` its command and output. */
  detail?: string;
}

/** One plan limit (5-hour, weekly…): `left` is the percent still available. */
export interface UsageWindow {
  kind: string;
  label: string;
  left: number;
  resetsAt: number | null;
  /** When the current pace would use it all up, if before the reset. */
  runsOutAt: number | null;
}

export interface UsageAccount {
  key: string;
  label: string;
  at: number;
  windows: UsageWindow[];
}

/** An agent's plan limits: `pooled` (several accounts combined) and each account's. */
export interface UsageProvider {
  id: string;
  name: string;
  accounts: UsageAccount[];
  pooled: { at: number; windows: UsageWindow[] } | null;
}

/** A desktop workspace, in sidebar order. */
export interface Workspace {
  id: string;
  name: string;
  /** Open on the desktop right now. */
  active: boolean;
}

/** An agent installed on the desktop. */
export interface Provider {
  id: string;
  name: string;
}

export type ThreadSource = "claude" | "codex" | "opencode" | "winfer" | "screen";

export type PhoneMsg =
  | { t: "hello"; device: string; ua: string }
  | { t: "panes.get" }
  | { t: "thread.get"; paneId: string; limit?: number }
  | { t: "send"; paneId: string; text: string }
  | { t: "approve" | "decline" | "interrupt"; paneId: string }
  | { t: "choose"; paneId: string; option: number; text?: string }
  | { t: "config.get"; paneId: string }
  | { t: "models.get"; provider: string }
  | { t: "usage.get" }
  | { t: "pane.set"; paneId: string; model?: string; effort?: string }
  | { t: "pane.new"; workspaceId: string; provider?: string; text?: string; model?: string; effort?: string }
  | { t: "pane.close"; paneId: string }
  | { t: "push.subscribe"; subscription: PushSubscriptionJSON }
  | { t: "push.unsubscribe" };

export type DesktopMsg =
  | { t: "welcome"; id?: string; desktop: string; version: string; vapidPublicKey: string }
  | { t: "panes"; id?: string; panes: Pane[]; workspaces?: Workspace[]; providers?: Provider[]; defaultProvider?: string; theme?: Theme; compactTools?: boolean; showBgNotes?: boolean }
  | { t: "status"; pane: Pane; silent?: boolean }
  | { t: "thread"; id?: string; paneId: string; messages: Message[]; source: ThreadSource }
  | { t: "config"; id?: string; paneId: string; models: ModelOption[] }
  | { t: "models"; id?: string; provider: string; models: ModelOption[]; defaultModel?: string | null; defaultEffort?: string | null; defaultEfforts?: string[] }
  | { t: "usage"; id?: string; providers: UsageProvider[]; now: number }
  | { t: "ack"; id: string; ok: boolean; error?: string; paneId?: string }
  | { t: "error"; id?: string; error: string };

export interface Presence {
  t: "presence";
  desktop: boolean;
  phones: number;
}

/** Inbox order: what needs you first. */
const RANK: Record<AgentStatus, number> = { "needs-input": 0, error: 1, done: 2, working: 3, idle: 4 };

export function sortPanes(panes: Pane[]): Pane[] {
  return [...panes].sort((a, b) => RANK[a.status] - RANK[b.status] || b.updatedAt - a.updatedAt);
}
