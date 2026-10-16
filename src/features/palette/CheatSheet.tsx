import { RotateCcw } from "lucide-react";
import { useEffect, useState } from "react";
import {
  BINDINGS,
  conflicts,
  eventChord,
  formatChord,
  IS_MAC,
  isCustomized,
  rowChord,
  setRecording,
  voiceChord,
  VOICE_BINDINGS,
  type Binding,
} from "@/commands/keymap";
import { cn } from "@/lib/cn";
import { useWorkspaces } from "@/stores/workspaces";

const GROUPS: (Binding["group"] | "Voice")[] = ["Panes", "Layout", "Workspaces", "App", "Voice"];

interface Row {
  id: string;
  title: string;
  /** Current chord, "#" for the digit of numbered families; null when switched off. */
  chord: string | null;
  numbered: boolean;
}

/** One row per binding; numbered bindings ("Focus pane 1…9") collapse into one. */
function rows(group: (typeof GROUPS)[number], overrides: Record<string, string>): Row[] {
  if (group === "Voice") return VOICE_BINDINGS.map((v) => ({ id: v.id, title: v.title, chord: voiceChord(v.id, overrides), numbered: false }));
  const seen = new Set<string>();
  return BINDINGS.filter((b) => b.group === group).flatMap((b) => {
    if (seen.has(b.id)) return [];
    seen.add(b.id);
    return [{ id: b.id, title: b.digit ? b.title.replace(/\d$/, "1–9") : b.title, chord: rowChord(b, overrides), numbered: Boolean(b.digit) }];
  });
}

const MODIFIERS = ["Ctrl", "Alt", "Meta"];

/** Turns a recorded key event into a chord for `row`, or an error to show. */
function recordChord(e: KeyboardEvent, row: Row): { chord: string } | { error: string } | null {
  const chord = eventChord(e);
  if (!chord) return null; // a lone modifier: keep waiting
  const parts = chord.split("+");
  const key = parts[parts.length - 1];
  if (!/^F\d+$/.test(key) && !parts.some((p) => MODIFIERS.includes(p))) return { error: `Use ${IS_MAC ? "⌃, ⌥ or ⌘" : "Ctrl, Alt or Win"} with the key (or an F-key).` };
  if (row.numbered) {
    if (!/^\d$/.test(key)) return { error: "Include a number key: the number picks the pane or workspace." };
    return { chord: [...parts.slice(0, -1), "#"].join("+") };
  }
  return { chord };
}

export function CheatSheet() {
  const { cheatSheetOpen, setCheatSheet, shortcuts, setShortcut, resetShortcuts } = useWorkspaces();
  const [editing, setEditing] = useState<Row | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [pending, setPending] = useState<{ row: Row; chord: string; clashes: { id: string; title: string }[] } | null>(null);

  useEffect(() => {
    if (!cheatSheetOpen) {
      setEditing(null);
      setPending(null);
      setMessage(null);
    }
  }, [cheatSheetOpen]);

  // While a row records, every key goes here: the keymap is paused and nothing else sees it.
  useEffect(() => {
    if (!editing) return;
    setRecording(true);
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (e.repeat) return;
      const plain = !e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey;
      if (plain && e.key === "Escape") return setEditing(null);
      if (plain && (e.key === "Backspace" || e.key === "Delete")) {
        setShortcut(editing.id, "");
        setMessage(`“${editing.title}” switched off.`);
        return setEditing(null);
      }
      const result = recordChord(e, editing);
      if (!result) return;
      if ("error" in result) return setMessage(result.error);
      const clashes = conflicts(editing.id, result.chord);
      if (clashes.length) setPending({ row: editing, chord: result.chord, clashes });
      else {
        setShortcut(editing.id, result.chord);
        setMessage(null);
      }
      setEditing(null);
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => {
      setRecording(false);
      window.removeEventListener("keydown", onKey, { capture: true });
    };
  }, [editing, setShortcut]);

  if (!cheatSheetOpen) return null;

  const replace = () => {
    if (!pending) return;
    pending.clashes.forEach((c) => setShortcut(c.id, ""));
    setShortcut(pending.row.id, pending.chord);
    setMessage(`Switched off: ${pending.clashes.map((c) => `“${c.title}”`).join(", ")}.`);
    setPending(null);
  };

  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex items-center justify-center bg-black/40" onMouseDown={() => setCheatSheet(false)}>
      <div className="anim-dialog flex max-h-[85vh] w-[820px] flex-col rounded-xl border border-border-strong bg-panel-raised shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-3 border-b border-border px-6 py-3">
          <h1 className="flex-1 text-sm font-semibold text-fg">Keyboard shortcuts</h1>
          <span className="text-xs text-fg-subtle">Click a shortcut to change it</span>
          {Object.keys(shortcuts).length > 0 && (
            <button onClick={resetShortcuts} className="rounded-md border border-border-strong px-2 py-1 text-xs text-fg-muted hover:bg-hover hover:text-fg">
              Reset all
            </button>
          )}
        </header>
        {(pending || message) && (
          <div className="flex items-center gap-2 border-b border-border px-6 py-2 text-xs">
            {pending ? (
              <>
                <span className="flex-1 text-warning">
                  {formatChord(pending.chord)} is used by {pending.clashes.map((c) => `“${c.title}”`).join(", ")}.
                </span>
                <button onClick={replace} className="rounded-md bg-accent px-2 py-1 text-on-accent hover:opacity-90">
                  Use it here instead
                </button>
                <button onClick={() => setPending(null)} className="rounded-md px-2 py-1 text-fg-muted hover:bg-hover">
                  Cancel
                </button>
              </>
            ) : (
              <span className="flex-1 text-fg-muted">{message}</span>
            )}
          </div>
        )}
        <div className="grid grid-cols-2 gap-x-8 gap-y-5 overflow-y-auto p-6">
          {GROUPS.map((group) => (
            <section key={group}>
              <h2 className="mb-2 text-xs font-medium tracking-wide text-fg-subtle uppercase">{group === "Voice" ? "Voice (works in any app)" : group}</h2>
              {rows(group, shortcuts).map((r) => {
                const recording = editing?.id === r.id;
                return (
                  <div key={r.id} className="group flex items-center gap-2 py-0.5 text-[13px]">
                    <span className="flex-1 text-fg-muted">{r.title}</span>
                    {isCustomized(r.id, shortcuts) && (
                      <button
                        title="Restore the default"
                        onClick={() => setShortcut(r.id, null)}
                        className="rounded p-0.5 text-fg-subtle opacity-0 group-hover:opacity-100 hover:text-fg"
                      >
                        <RotateCcw size={11} />
                      </button>
                    )}
                    <button
                      onClick={() => {
                        setPending(null);
                        setMessage(`Press the new keys${r.numbered ? ` with any number, e.g. ${formatChord("Ctrl+Alt+1")}` : ""}. Esc cancels, ${IS_MAC ? "⌫" : "Backspace"} switches it off.`);
                        setEditing(recording ? null : r);
                      }}
                      className={cn(
                        "min-w-16 rounded-md border px-1.5 py-0.5 text-right font-mono text-[11px]",
                        recording ? "border-accent text-accent" : "border-transparent text-fg hover:border-border-strong",
                        !r.chord && !recording && "text-fg-subtle",
                      )}
                    >
                      {recording ? "Press keys…" : r.chord ? formatChord(r.chord) : "off"}
                    </button>
                  </div>
                );
              })}
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
