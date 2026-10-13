import { ExternalLink, GitBranch, Maximize2, Minimize2, Plus, X } from "lucide-react";
import { execute } from "@/commands/bus";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { StatusDot } from "./StatusDot";
import { PaneMenu } from "./PaneMenu";
import { TerminalHost } from "./TerminalHost";
import type { Pane } from "@/stores/workspaces";

const HeaderButton = ({ title, onClick, children }: { title: string; onClick?: () => void; children: React.ReactNode }) => (
  <button
    title={title}
    onClick={onClick}
    onMouseDown={(e) => e.stopPropagation()}
    className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg"
  >
    {children}
  </button>
);

interface Props {
  pane: Pane;
  /** 1-based position in layout order; matches ⌘1–9 and voice "focus 3". */
  index: number;
  focused: boolean;
  zoomed: boolean;
  onFocus: () => void;
}

export function PaneFrame({ pane, index, focused, zoomed, onFocus }: Props) {
  const target = { id: pane.id };

  return (
    <section
      data-pane-id={pane.id}
      onMouseDown={onFocus}
      className={cn(
        "flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border bg-panel",
        focused ? "border-accent/70" : "border-border",
      )}
    >
      <header className="flex h-8 shrink-0 items-center gap-2 border-b border-border px-3">
        <span className="min-w-3 font-mono text-[11px] text-fg-subtle">{index}</span>
        <StatusDot status={pane.status} />
        <ProviderIcon provider={pane.provider} />
        <span className="flex-1 truncate font-mono text-xs text-fg-muted">{pane.title}</span>
        {pane.worktree && (
          <button
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => execute({ type: "pane.showChanges", target })}
            title={`${pane.worktree.path}\nClick to show changes`}
            className="flex max-w-40 items-center gap-1 truncate rounded px-1.5 py-0.5 font-mono text-[11px] text-fg-subtle hover:bg-hover hover:text-fg"
          >
            <GitBranch size={11} />
            <span className="truncate">{pane.worktree.branch.replace(/^winfer\//, "")}</span>
          </button>
        )}
        {pane.devUrl && (
          <button
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => execute({ type: "browser.open", url: pane.devUrl! })}
            title={`Open ${pane.devUrl} in the browser panel`}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[11px] text-accent hover:bg-accent-soft"
          >
            {pane.devUrl.replace(/^https?:\/\//, "").replace(/\/$/, "")}
            <ExternalLink size={10} />
          </button>
        )}
        <PaneMenu pane={pane} />
        <HeaderButton title={zoomed ? "Restore (⌘↩)" : "Zoom (⌘↩)"} onClick={() => execute({ type: "pane.zoom", target })}>
          {zoomed ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
        </HeaderButton>
        <HeaderButton
          title="Split right (⌘D)"
          onClick={() => {
            onFocus();
            execute({ type: "pane.split", dir: "row" });
          }}
        >
          <Plus size={14} />
        </HeaderButton>
        <HeaderButton title="Close (⌘W)" onClick={() => execute({ type: "pane.close", target })}>
          <X size={14} />
        </HeaderButton>
      </header>

      <div className="min-h-0 flex-1 py-1.5 pl-2.5 pr-1">
        <TerminalHost pane={pane} focused={focused} />
      </div>
    </section>
  );
}
