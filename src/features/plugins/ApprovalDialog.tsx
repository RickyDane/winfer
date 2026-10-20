/** The in-app dialog for plugins' `api.confirm` questions (see approvals.ts). */
import { ShieldQuestion, TriangleAlert } from "lucide-react";
import { useEffect, useRef } from "react";
import { answerApproval, useApprovals } from "./approvals";
import { usePlugins } from "./plugins";

export function ApprovalDialog() {
  const approval = useApprovals((s) => s.queue[0]);
  const waiting = useApprovals((s) => s.queue.length - 1);
  const pluginName = usePlugins((s) => s.plugins.find((p) => p.id === approval?.pluginId)?.manifest?.name ?? approval?.pluginId);
  const okRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (approval) okRef.current?.focus();
  }, [approval]);

  if (!approval) return null;
  const Icon = approval.kind === "warning" ? TriangleAlert : ShieldQuestion;
  return (
    <div className="anim-backdrop fixed inset-0 z-[60] flex items-center justify-center bg-black/40">
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="wf-approval-title"
        className="anim-dialog flex max-h-[70vh] w-[460px] flex-col rounded-xl border border-border-strong bg-panel-raised shadow-2xl"
      >
        <header className="flex items-start gap-3 border-b border-border px-5 py-3">
          <Icon size={16} className={approval.kind === "warning" ? "mt-0.5 shrink-0 text-warning" : "mt-0.5 shrink-0 text-accent"} />
          <div className="min-w-0 flex-1">
            <h1 id="wf-approval-title" className="text-sm font-semibold text-fg">
              {approval.title}
            </h1>
            <p className="text-xs text-fg-subtle">
              {pluginName}
              {approval.paneId ? " · you can also answer from your phone" : ""}
            </p>
          </div>
        </header>
        <p className="overflow-y-auto whitespace-pre-wrap break-words px-5 py-4 text-[13px] text-fg select-text">{approval.message}</p>
        <footer className="flex items-center gap-2 border-t border-border px-5 py-3">
          {waiting > 0 && <span className="text-xs text-fg-subtle">{waiting} more waiting</span>}
          <span className="flex-1" />
          <button onClick={() => answerApproval(approval.id, false)} className="rounded-md px-2.5 py-1 text-xs text-fg-muted hover:bg-hover hover:text-fg">
            Cancel
          </button>
          <button ref={okRef} onClick={() => answerApproval(approval.id, true)} className="rounded-md bg-accent px-3 py-1 text-xs font-medium text-on-accent hover:opacity-90">
            {approval.okLabel}
          </button>
        </footer>
      </div>
    </div>
  );
}
