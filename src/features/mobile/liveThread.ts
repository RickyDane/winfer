import type { CodeState } from "@/features/code/store";
import type { MobileMessage } from "./mobile";

/** Read the same in-memory items as the desktop, including incomplete streamed replies. */
export function liveThread(state: CodeState, limit: number): MobileMessage[] {
  const messages: MobileMessage[] = [];
  for (const item of state.items) {
    const base = { id: item.key, ts: null };
    if (item.kind === "user") messages.push({ ...base, role: "user", text: item.text, ts: item.at || null });
    if (item.kind === "text") messages.push({ ...base, role: "assistant", text: item.text });
    if (item.kind === "tool") {
      const task = Object.values(state.tasks).find((t) => t.tool_id === item.key);
      messages.push({
        ...base, role: "tool", text: item.title || item.name, tool: item.name, ts: item.started || null,
        state: task?.status === "running" ? "running" : task?.status === "failed" ? "failed" : item.ended === undefined ? "running" : item.ok === false ? "failed" : "done",
        background: !!task,
      });
    }
  }
  const last = state.items.at(-1);
  if (state.busy && last?.kind === "reasoning" && last.streaming) messages.push({ id: last.key, role: "thinking", text: "", ts: last.started });
  return messages.slice(-limit);
}
