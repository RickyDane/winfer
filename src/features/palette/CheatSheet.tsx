import { useEffect } from "react";
import { BINDINGS, formatKeys, type Binding } from "@/commands/keymap";
import { useWorkspaces } from "@/stores/workspaces";

const GROUPS: Binding["group"][] = ["Panes", "Layout", "Workspaces", "App"];

/** Collapses numbered bindings ("Focus pane 1…9") into one row. */
function rows(group: Binding["group"]) {
  const seen = new Set<string>();
  return BINDINGS.filter((b) => b.group === group).flatMap((b) => {
    const numbered = /\d$/.test(b.title);
    const title = numbered ? b.title.replace(/\d$/, "1–9") : b.title;
    if (seen.has(title)) return [];
    seen.add(title);
    const keys = numbered ? formatKeys(b).replace(/\d$/, "1–9") : formatKeys(b);
    return [{ title, keys }];
  });
}

export function CheatSheet() {
  const { cheatSheetOpen, setCheatSheet } = useWorkspaces();

  useEffect(() => {
    if (!cheatSheetOpen) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setCheatSheet(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [cheatSheetOpen, setCheatSheet]);

  if (!cheatSheetOpen) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onMouseDown={() => setCheatSheet(false)}>
      <div
        className="grid max-h-[80vh] w-[760px] grid-cols-2 gap-x-8 gap-y-5 overflow-y-auto rounded-xl border border-border-strong bg-panel-raised p-6 shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        {GROUPS.map((group) => (
          <section key={group}>
            <h2 className="mb-2 text-xs font-medium tracking-wide text-fg-subtle uppercase">{group}</h2>
            {rows(group).map((r) => (
              <div key={r.title} className="flex items-center justify-between py-1 text-[13px]">
                <span className="text-fg-muted">{r.title}</span>
                <kbd className="font-mono text-[11px] text-fg">{r.keys}</kbd>
              </div>
            ))}
          </section>
        ))}
      </div>
    </div>
  );
}
