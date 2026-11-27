import type { Message, Pane } from "../protocol";

/** Whether every word of `query` is somewhere in the agent: its workspace, session or what was said (`thread`: if loaded). */
export function matchesPane(pane: Pane, query: string, agent?: string, thread: Message[] = []): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const text = [
    pane.title,
    pane.workspace,
    pane.machineName,
    agent,
    pane.message,
    pane.reply,
    pane.request?.text,
    ...(pane.request?.options?.map((o) => o.label) ?? []),
    ...thread.flatMap((m) => [m.text, m.desc, m.detail]),
  ]
    .filter(Boolean)
    .join("\n")
    .toLowerCase();
  return words.every((w) => text.includes(w));
}
