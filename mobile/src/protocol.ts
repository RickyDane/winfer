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
}

export interface Message {
  id: string;
  /** "thinking": a text-less marker ending the thread while the agent reasons; never shown. */
  role: "user" | "assistant" | "tool" | "thinking";
  text: string;
  tool?: string;
  ts: number | null;
  /** Tools: whether the call is still running, when the session says. */
  state?: "running" | "done" | "failed";
  /** Tools: a background shell or subagent, which keeps running after its call returns. */
  background?: boolean;
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
  | { t: "pane.new"; workspaceId: string; provider?: string; text?: string }
  | { t: "pane.close"; paneId: string }
  | { t: "push.subscribe"; subscription: PushSubscriptionJSON }
  | { t: "push.unsubscribe" };

export type DesktopMsg =
  | { t: "welcome"; id?: string; desktop: string; version: string; vapidPublicKey: string }
  | { t: "panes"; id?: string; panes: Pane[]; workspaces?: Workspace[]; providers?: Provider[]; defaultProvider?: string }
  | { t: "status"; pane: Pane; silent?: boolean }
  | { t: "thread"; id?: string; paneId: string; messages: Message[]; source: ThreadSource }
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
