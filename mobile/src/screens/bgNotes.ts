import { useSyncExternalStore } from "react";

/* Phone-side preference for background task rows (defaults to desktop's showBgNotes setting). */
const PREF_KEY = "winfer.showBgNotes";
const listeners = new Set<() => void>();

function readOverride(): boolean | null {
  try {
    const v = localStorage.getItem(PREF_KEY);
    if (v === "true") return true;
    if (v === "false") return false;
  } catch {
    /* private mode / node test */
  }
  return null;
}

let cachedOverride: boolean | null = readOverride();

export function setShowBgNotesOverride(value: boolean) {
  cachedOverride = value;
  try {
    localStorage.setItem(PREF_KEY, value ? "true" : "false");
  } catch {
    /* private mode */
  }
  listeners.forEach((fn) => fn());
}

const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => void listeners.delete(fn);
};

export function useShowBgNotes(desktopDefault: boolean): boolean {
  const override = useSyncExternalStore(subscribe, () => cachedOverride, () => null);
  return override ?? desktopDefault;
}
