import { Download, Loader2, Sparkles, X } from "lucide-react";
import { useEffect } from "react";
import { parseChanges, splitArea } from "./releaseNotes";
import { closeReleaseNotes, installUpdate, useUpdates } from "./updater";

/**
 * The changes in a release: before installing an available update ("Install and restart"),
 * and once on the first launch after an update.
 */
export function WhatsNew() {
  const { notesOpen, available, installed, installing, version } = useUpdates();
  const release =
    notesOpen === "available" && available
      ? { version: available.version, changes: parseChanges(available.body) }
      : notesOpen === "installed"
        ? installed
        : null;

  useEffect(() => {
    if (!release) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && closeReleaseNotes();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [release]);

  if (!release) return null;
  const upcoming = notesOpen === "available";

  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex items-center justify-center bg-black/40" onMouseDown={closeReleaseNotes}>
      <div
        className="anim-dialog flex max-h-[70vh] w-[460px] flex-col rounded-xl border border-border-strong bg-panel-raised shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="flex items-start gap-3 border-b border-border px-5 py-3">
          <Sparkles size={16} className="mt-0.5 shrink-0 text-accent" />
          <div className="min-w-0 flex-1">
            <h1 className="text-sm font-semibold text-fg">{upcoming ? `winfer ${release.version} is available` : `What's new in winfer ${release.version}`}</h1>
            <p className="text-xs text-fg-subtle">{upcoming ? `You have ${version ?? "?"}. Restarting stops the agents running in your panes.` : "Here's what changed since your last version."}</p>
          </div>
          <button onClick={closeReleaseNotes} className="shrink-0 rounded p-0.5 text-fg-subtle hover:bg-hover hover:text-fg" title="Close">
            <X size={14} />
          </button>
        </header>
        <ul className="flex flex-col gap-2 overflow-y-auto px-5 py-4">
          {release.changes.length === 0 && <li className="text-[13px] text-fg-muted">No notes for this release.</li>}
          {release.changes.map((change, i) => {
            const { area, text } = splitArea(change);
            return (
              <li key={i} className="flex items-baseline gap-2 text-[13px] text-fg">
                <span className="mt-1.5 size-1.5 shrink-0 self-start rounded-full bg-accent/70" />
                <span className="min-w-0 select-text">
                  {area && <span className="mr-1.5 rounded bg-hover px-1.5 py-px text-[11px] font-medium text-fg-muted">{area}</span>}
                  {text.charAt(0).toUpperCase() + text.slice(1)}
                </span>
              </li>
            );
          })}
        </ul>
        <footer className="flex items-center justify-end gap-2 border-t border-border px-5 py-3">
          {upcoming ? (
            <>
              <button onClick={closeReleaseNotes} className="rounded-md px-2.5 py-1 text-xs text-fg-muted hover:bg-hover hover:text-fg">
                Later
              </button>
              <button
                onClick={() => void installUpdate()}
                disabled={installing}
                className="flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-on-accent hover:opacity-90 disabled:opacity-50"
              >
                {installing ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
                Install and restart
              </button>
            </>
          ) : (
            <button onClick={closeReleaseNotes} className="rounded-md bg-accent px-3 py-1 text-xs font-medium text-on-accent hover:opacity-90">
              Got it
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}
