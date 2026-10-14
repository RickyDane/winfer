import { create } from "zustand";

/** Short status messages in the bottom corner: "Merging…" → "Merged feat/x into main". */
export type ToastKind = "info" | "success" | "error" | "loading";

export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
  detail?: string;
  /** A button on the toast (it stays up longer). */
  action?: { label: string; run: () => void };
}

interface ToastState {
  toasts: Toast[];
}

export const useToasts = create<ToastState>(() => ({ toasts: [] }));

let nextId = 1;
const timers = new Map<number, number>();

const LIFETIME: Record<ToastKind, number | null> = { info: 2500, success: 3000, error: 7000, loading: null };

export function dismissToast(id: number) {
  clearTimeout(timers.get(id));
  timers.delete(id);
  useToasts.setState((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
}

function schedule(id: number, kind: ToastKind, hasAction = false) {
  clearTimeout(timers.get(id));
  const ms = hasAction ? 20_000 : LIFETIME[kind];
  if (ms !== null) timers.set(id, window.setTimeout(() => dismissToast(id), ms));
}

/** Shows a toast; returns its id so a "loading" one can be updated or dismissed. */
export function toast(text: string, kind: ToastKind = "info", detail?: string, action?: Toast["action"]): number {
  const id = nextId++;
  // Keep the stack short; the oldest ones go first.
  useToasts.setState((s) => ({ toasts: [...s.toasts, { id, kind, text, detail, action }].slice(-4) }));
  schedule(id, kind, !!action);
  return id;
}

export function updateToast(id: number, patch: Partial<Omit<Toast, "id">>) {
  const t = useToasts.getState().toasts.find((x) => x.id === id);
  if (!t) {
    // Dismissed meanwhile (or pushed out): show the outcome as a new one.
    if (patch.text) toast(patch.text, patch.kind, patch.detail);
    return;
  }
  useToasts.setState((s) => ({ toasts: s.toasts.map((x) => (x.id === id ? { ...x, ...patch } : x)) }));
  if (patch.kind) schedule(id, patch.kind);
}

/**
 * A spinner toast while `work` runs, then its result. `error` false keeps failures quiet
 * (for callers that report them their own way).
 */
export async function withToast<T>(
  work: Promise<T>,
  text: { loading: string; success: string | ((value: T) => string); error?: string | false },
): Promise<T> {
  const id = toast(text.loading, "loading");
  try {
    const value = await work;
    updateToast(id, { kind: "success", text: typeof text.success === "function" ? text.success(value) : text.success });
    return value;
  } catch (err) {
    if (text.error === false) dismissToast(id);
    else updateToast(id, { kind: "error", text: text.error ?? "Failed", detail: String(err) });
    throw err;
  }
}

/** Copies `text` and confirms it. */
export function copyWithToast(text: string, what = "Copied") {
  navigator.clipboard.writeText(text).then(
    () => toast(what, "success", text),
    (e) => toast("Couldn't copy", "error", String(e)),
  );
}
