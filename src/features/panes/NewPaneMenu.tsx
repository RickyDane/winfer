import { ChevronRight, Loader2, Plus, Star } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { UsageHint } from "@/features/usage/UsageMeter";
import { defaultProviderId, useWorkspaces } from "@/stores/workspaces";

/** Provider picker for opening a new pane; the star marks (and sets) the default agent. */
export function NewPaneMenu() {
  const { providers, addPane, models, loadModels, agentModels, defaultProvider, setDefaultProvider } = useWorkspaces();
  const defaultId = defaultProviderId({ providers, defaultProvider });
  const [open, setOpen] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);

  return (
    <div ref={root} className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 rounded-md border border-border px-2 py-1 text-xs hover:bg-hover hover:text-fg"
        title="New pane"
      >
        <Plus size={13} />
        <span>New</span>
      </button>
      {open && (
        <div className="anim-menu absolute right-0 z-50 mt-1 w-64 rounded-lg border border-border-strong bg-panel-raised p-1 shadow-xl">
          {providers.map((p) => (
            <div key={p.id}>
              <div className="flex items-center">
                <button
                  disabled={!p.installed}
                  onClick={() => {
                    addPane(p.id);
                    setOpen(false);
                  }}
                  className={cn(
                    "flex flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px]",
                    p.installed ? "text-fg hover:bg-hover" : "cursor-not-allowed text-fg-subtle",
                  )}
                >
                  <ProviderIcon provider={p.id} />
                  <span className="flex-1">{p.name}</span>
                  {!p.installed && <span className="text-[11px]">not installed</span>}
                  {p.installed && agentModels[p.id] && <span className="font-mono text-[10px] text-fg-subtle">{agentModels[p.id]}</span>}
                  {p.installed && <UsageHint provider={p.id} />}
                </button>
                {p.installed && (
                  <button
                    title={p.id === defaultId ? "Default agent for new panes" : "Make this the default agent for new panes"}
                    onClick={() => setDefaultProvider(p.id)}
                    className={cn("rounded p-1 hover:bg-hover", p.id === defaultId ? "text-accent" : "text-fg-subtle/40 hover:text-fg")}
                  >
                    <Star size={12} fill={p.id === defaultId ? "currentColor" : "none"} />
                  </button>
                )}
                {p.installed && p.modelArg && (models[p.id]?.length ?? 1) > 0 && (
                  <button
                    title="Choose a model"
                    onClick={() => {
                      setExpanded(expanded === p.id ? null : p.id);
                      void loadModels(p.id);
                    }}
                    className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg"
                  >
                    <ChevronRight size={12} className={cn("transition-transform", expanded === p.id && "rotate-90")} />
                  </button>
                )}
              </div>
              {expanded === p.id && (
                <div className="mb-1 ml-6 max-h-48 overflow-y-auto border-l border-border pl-1">
                  {(models[p.id] ?? []).map((m) => (
                    <button
                      key={m.id}
                      onClick={() => {
                        addPane(p.id, { model: m.id });
                        setOpen(false);
                      }}
                      className="block w-full truncate rounded px-2 py-1 text-left text-xs text-fg-muted hover:bg-hover hover:text-fg"
                    >
                      {m.label}
                    </button>
                  ))}
                  {!models[p.id] && (
                    <div className="flex items-center gap-1.5 px-2 py-1 text-xs text-fg-subtle">
                      <Loader2 size={11} className="animate-spin" /> Loading models…
                    </div>
                  )}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
