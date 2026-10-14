import { CheckCircle2, Info, Loader2, X, XCircle } from "lucide-react";
import { cn } from "@/lib/cn";
import { dismissToast, useToasts, type ToastKind } from "@/stores/toasts";

const ICON: Record<ToastKind, React.ReactNode> = {
  loading: <Loader2 size={14} className="animate-spin text-accent" />,
  success: <CheckCircle2 size={14} className="text-success" />,
  error: <XCircle size={14} className="text-danger" />,
  info: <Info size={14} className="text-fg-muted" />,
};

/** Bottom-right stack of toasts (see stores/toasts). */
export function Toaster() {
  const toasts = useToasts((s) => s.toasts);
  if (toasts.length === 0) return null;
  return (
    <div className="pointer-events-none fixed right-4 bottom-4 z-[80] flex w-80 flex-col items-end gap-2" role="status" aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={cn(
            "anim-rise pointer-events-auto flex w-full items-start gap-2 rounded-lg border bg-panel-raised px-3 py-2 shadow-xl",
            t.kind === "error" ? "border-danger/40" : "border-border-strong",
          )}
        >
          <span className="mt-0.5 shrink-0">{ICON[t.kind]}</span>
          <div className="min-w-0 flex-1">
            <div className="text-[13px] text-fg">{t.text}</div>
            {t.detail && <div className="mt-0.5 line-clamp-3 font-mono text-[11px] break-all text-fg-subtle select-text">{t.detail}</div>}
            {t.action && (
              <button
                onClick={() => {
                  dismissToast(t.id);
                  t.action!.run();
                }}
                className="mt-1.5 rounded-md bg-accent px-2 py-0.5 text-xs font-medium text-on-accent hover:opacity-90"
              >
                {t.action.label}
              </button>
            )}
          </div>
          {t.kind !== "loading" && (
            <button onClick={() => dismissToast(t.id)} className="shrink-0 rounded p-0.5 text-fg-subtle hover:bg-hover hover:text-fg" title="Dismiss">
              <X size={12} />
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
