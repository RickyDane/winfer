/** Each pane's last agent reply, from the agent's "done" hook (Claude Code, Codex). */
const replies = new Map<string, string>();

export function setLastReply(paneId: string, reply: string | null | undefined) {
  if (reply) replies.set(paneId, reply);
  else replies.delete(paneId);
}

export const lastReply = (paneId: string): string | null => replies.get(paneId) ?? null;
