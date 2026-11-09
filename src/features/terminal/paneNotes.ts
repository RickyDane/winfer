import { create } from "zustand";

const KEY = "winfer-pane-notes";

function load(): Record<string, string> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    if (value && typeof value === "object" && !Array.isArray(value)) {
      return Object.fromEntries(Object.entries(value).filter(([, note]) => typeof note === "string"));
    }
  } catch { /* Storage may be unavailable. Notes still work for this session. */ }
  return {};
}

/** User-only UI state: deliberately separate from Pane, prompts and agent APIs. */
export const usePaneNotes = create<{ notes: Record<string, string>; setNote: (paneId: string, note: string) => boolean }>((set, get) => ({
  notes: load(),
  setNote: (paneId, note) => {
    const notes = { ...get().notes };
    if (note) notes[paneId] = note;
    else delete notes[paneId];
    set({ notes });
    try {
      localStorage.setItem(KEY, JSON.stringify(notes));
      return true;
    } catch {
      return false;
    }
  },
}));
