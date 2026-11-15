import type { StoreApi } from "zustand";
import type { CodeState } from "./store";

const BRACKETED_PASTE = /\x1b\[20[01]~/g;

/**
 * Text typed into a winfer code pane from outside it (the agent bar's broadcast, send_to_pane,
 * the phone, Approve and Interrupt), read the way its terminal UI reads keys: text goes into the
 * composer, Enter sends what's there, Esc interrupts and "y" accepts a pending approval.
 */
export function typeIntoCode(store: StoreApi<CodeState>, data: string) {
  const s = store.getState();
  if (data === "\x1b") {
    s.send({ type: "interrupt" });
    return;
  }
  const approval = s.approvals[0];
  if (data === "y" && approval && !approval.questions.length && !s.draft) {
    s.send({ type: "approve", id: approval.id, decision: "allow" });
    return;
  }
  const parts = data.replace(BRACKETED_PASTE, "").split("\r");
  parts.forEach((part, i) => {
    if (part) store.getState().setDraft(store.getState().draft + part);
    if (i === parts.length - 1) return;
    const { draft, submit, setDraft } = store.getState();
    if (!draft.trim()) return;
    submit(draft);
    setDraft("");
  });
}
