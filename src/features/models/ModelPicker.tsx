import { Check, Cpu } from "lucide-react";
import { Spinner } from "@/components/Spinner";
import { useEffect, useMemo, useState } from "react";
import { execute } from "@/commands/bus";
import { cn } from "@/lib/cn";
import { providerName, useWorkspacesPick } from "@/stores/workspaces";

/** Picks the model for the focused agent. Lists what the CLI offers; any typed name works too. */
export function ModelPicker() {
  const { overlay, setOverlay, focusedPaneId, workspaces, providers, models, loadModels } = useWorkspacesPick("overlay", "setOverlay", "focusedPaneId", "workspaces", "providers", "models", "loadModels");
  const pane = workspaces.flatMap((w) => w.panes).find((p) => p.id === focusedPaneId);
  const provider = providers.find((p) => p.id === pane?.provider);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const list = (pane && models[pane.provider]) ?? [];

  useEffect(() => {
    if (overlay !== "model" || !pane) return;
    setQuery("");
    if (!models[pane.provider]) {
      setLoading(true);
      void loadModels(pane.provider).finally(() => setLoading(false));
    }
  }, [overlay, pane?.provider]); // eslint-disable-line react-hooks/exhaustive-deps

  const filtered = useMemo(() => {
    const q = query.toLowerCase();
    return list.filter((m) => m.id.toLowerCase().includes(q) || m.label.toLowerCase().includes(q)).slice(0, 200);
  }, [list, query]);

  if (overlay !== "model") return null;
  const close = () => setOverlay(null);
  const choose = (model: string | null) => {
    if (pane) execute({ type: "pane.setModel", target: { id: pane.id }, model });
    close();
  };

  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex justify-center bg-black/40 pt-[14vh]" onMouseDown={close}>
      <div className="anim-dialog flex h-fit max-h-[60vh] w-[480px] flex-col overflow-hidden rounded-xl border border-border-strong bg-panel-raised shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        {!pane || !provider?.modelArg ? (
          <div className="p-5 text-xs text-fg-muted">{pane ? `${providerName(providers, pane.provider)} can't switch models.` : "Focus an agent pane first."}</div>
        ) : (
          <>
            <div className="flex items-center gap-2 border-b border-border px-4 py-2.5 text-xs text-fg-subtle">
              <Cpu size={13} />
              <span>
                Model for {providerName(providers, pane.provider)}
                {provider.modelCommand ? " · switches in place" : " · restarts and resumes the session"}
              </span>
            </div>
            <input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") close();
                if (e.key === "Enter" && query.trim()) choose(filtered[0]?.id ?? query.trim());
              }}
              placeholder={list.length ? "Search models, or type a model name" : "Type a model name"}
              className="h-10 border-b border-border bg-transparent px-4 font-mono text-xs text-fg outline-none placeholder:text-fg-subtle"
            />
            <div className="overflow-y-auto p-1.5">
              {loading && (
                <div className="flex items-center gap-2 px-3 py-2 text-xs text-fg-subtle">
                  <Spinner size={12} /> Asking {provider.name} for its models…
                </div>
              )}
              <ModelRow label="CLI default" id={null} active={!pane.model} onPick={choose} />
              {filtered.map((m) => (
                <ModelRow key={m.id} label={m.label} id={m.id} active={pane.model === m.id} onPick={choose} />
              ))}
              {query.trim() && !list.some((m) => m.id === query.trim()) && (
                <ModelRow label={`Use “${query.trim()}”`} id={query.trim()} active={false} onPick={choose} />
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function ModelRow({ label, id, active, onPick }: { label: string; id: string | null; active: boolean; onPick: (id: string | null) => void }) {
  return (
    <button onClick={() => onPick(id)} className={cn("flex w-full items-center gap-2 rounded-md px-3 py-1.5 text-left text-[13px] hover:bg-hover", active ? "text-fg" : "text-fg-muted")}>
      <span className="flex-1 truncate">{label}</span>
      {id && id !== label && <span className="truncate font-mono text-[11px] text-fg-subtle">{id}</span>}
      {active && <Check size={13} className="text-accent" />}
    </button>
  );
}
