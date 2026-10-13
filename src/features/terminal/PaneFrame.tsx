import { Ellipsis, Maximize2, Plus, X } from "lucide-react";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { StatusDot } from "./StatusDot";
import { TerminalHost } from "./TerminalHost";
import { closeTerminal } from "./xtermRegistry";
import { useWorkspaces, type Pane } from "@/stores/workspaces";

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

export function PaneFrame({ pane, focused, onFocus }: { pane: Pane; focused: boolean; onFocus: () => void }) {
  const { addPane, removePane } = useWorkspaces();

  const close = () => {
    closeTerminal(pane.id);
    removePane(pane.id);
  };

  return (
    <section
      onMouseDown={onFocus}
      className={cn(
        "flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-[var(--radius-pane)] border bg-panel",
        focused ? "border-accent/70" : "border-border",
      )}
    >
      <header className="flex h-8 shrink-0 items-center gap-2 border-b border-border px-3">
        <StatusDot status={pane.status} />
        <ProviderIcon provider={pane.provider} />
        <span className="flex-1 truncate font-mono text-xs text-fg-muted">{pane.title}</span>
        <HeaderButton title="More">
          <Ellipsis size={14} />
        </HeaderButton>
        <HeaderButton title="Zoom (⌘↩)">
          <Maximize2 size={13} />
        </HeaderButton>
        <HeaderButton title="Split with a shell (⌘D)" onClick={() => addPane("shell", { splitFrom: pane.id })}>
          <Plus size={14} />
        </HeaderButton>
        <HeaderButton title="Close (⌘W)" onClick={close}>
          <X size={14} />
        </HeaderButton>
      </header>

      <div className="min-h-0 flex-1 py-1.5 pl-2.5 pr-1">
        <TerminalHost pane={pane} focused={focused} />
      </div>
    </section>
  );
}
