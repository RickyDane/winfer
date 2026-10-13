import { create } from "zustand";

export interface ActivityEvent {
  id: number;
  at: number;
  workspaceId: string;
  /** Provider id for the icon, if the event is about a pane. */
  provider: string | null;
  text: string;
}

interface ActivityState {
  events: ActivityEvent[];
}

const MAX_EVENTS = 300;
let nextId = 1;

export const useActivity = create<ActivityState>(() => ({ events: [] }));

/** Records something that happened, for the sidebar's "Latest" feed. */
export function logActivity(workspaceId: string, text: string, provider: string | null = null) {
  useActivity.setState((s) => ({
    events: [{ id: nextId++, at: Date.now(), workspaceId, provider, text }, ...s.events].slice(0, MAX_EVENTS),
  }));
}

export function relativeTime(at: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - at) / 1000));
  if (s < 45) return "now";
  if (s < 3600) return `${Math.round(s / 60)}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}
