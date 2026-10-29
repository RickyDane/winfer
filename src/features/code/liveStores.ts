import type { StoreApi } from "zustand";
import type { CodeState } from "./store";

/** Mounted sessions remain live across workspace switches; never start a session just to read it. */
export const liveCodeStores = new Map<string, StoreApi<CodeState>>();
