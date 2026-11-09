import { create } from "zustand";

interface UndockState {
  /** The undocked window, null while the browser sits in the side panel. */
  win: Window | null;
  /** The element the panel renders into. */
  root: HTMLElement | null;
}

export const useUndock = create<UndockState>(() => ({ win: null, root: null }));
