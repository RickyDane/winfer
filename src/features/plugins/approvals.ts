/**
 * Plugins' `api.confirm` questions. They show as an in-app dialog (not a native one, which
 * can't be closed from code) and, when an agent's tool call asked, as that pane's approval on
 * the paired phones, so either side can answer.
 */
import { create } from "zustand";

export interface PluginApproval {
  id: number;
  pluginId: string;
  /** The pane whose agent asked; null for questions from a plugin's own UI. */
  paneId: string | null;
  title: string;
  message: string;
  okLabel: string;
  kind: "info" | "warning";
  resolve: (ok: boolean) => void;
}

export const useApprovals = create<{ queue: PluginApproval[] }>(() => ({ queue: [] }));

let serial = 0;

export function requestApproval(spec: Omit<PluginApproval, "id" | "resolve">): Promise<boolean> {
  return new Promise((resolve) => {
    const approval: PluginApproval = { ...spec, id: ++serial, resolve };
    useApprovals.setState((s) => ({ queue: [...s.queue, approval] }));
  });
}

/** Answers a pending question; false if it was already answered. */
export function answerApproval(id: number, ok: boolean): boolean {
  const approval = useApprovals.getState().queue.find((a) => a.id === id);
  if (!approval) return false;
  useApprovals.setState((s) => ({ queue: s.queue.filter((a) => a !== approval) }));
  approval.resolve(ok);
  return true;
}

/** The oldest question an agent in `paneId` is waiting on. */
export const approvalFor = (paneId: string) => useApprovals.getState().queue.find((a) => a.paneId === paneId);
