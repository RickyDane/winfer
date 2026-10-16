import { BookOpen, Play, Trash2, X } from "lucide-react";
import { useState } from "react";
import { useActiveWorkspace, useWorkspaces } from "@/stores/workspaces";
import { BUILT_IN, allRecipes, deleteRecipe, describeRecipe, loadRecipe, saveCurrentAsRecipe } from "./recipes";

export function RecipesView() {
  const { overlay, setOverlay, recipes: saved } = useWorkspaces();
  const workspace = useActiveWorkspace();
  const [name, setName] = useState("");

  if (overlay !== "recipes") return null;
  const recipes = allRecipes();
  const builtIn = saved.length === 0;

  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex justify-center bg-black/40 pt-[14vh]" onMouseDown={() => setOverlay(null)}>
      <div className="anim-dialog h-fit w-[560px] space-y-4 rounded-xl border border-border-strong bg-panel-raised p-5 shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-2">
          <BookOpen size={14} className="text-accent" />
          <h1 className="flex-1 text-sm font-semibold text-fg">Recipes</h1>
          <button onClick={() => setOverlay(null)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>
        <div className="divide-y divide-border rounded-lg border border-border">
          {recipes.map((r) => (
            <div key={r.id} className="flex items-center gap-3 px-3 py-2 text-xs">
              <div className="min-w-0 flex-1">
                <div className="text-[13px] text-fg">{r.name}</div>
                <div className="truncate text-fg-subtle">
                  {describeRecipe(r)} · {r.preset}
                </div>
              </div>
              <button
                onClick={() => {
                  setOverlay(null);
                  loadRecipe(r.id);
                }}
                className="flex items-center gap-1 rounded-md bg-accent-soft px-2 py-1 text-accent hover:bg-accent hover:text-on-accent"
              >
                <Play size={11} /> Load
              </button>
              {saved.some((own) => own.id === r.id) && (
                <button onClick={() => deleteRecipe(r.id)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-danger" title="Delete">
                  <Trash2 size={12} />
                </button>
              )}
            </div>
          ))}
        </div>
        {builtIn && <p className="text-[11px] text-fg-subtle">These are examples ({BUILT_IN.length}). Save your own and they replace them.</p>}
        <form
          className="flex items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (!name.trim()) return;
            saveCurrentAsRecipe(name.trim());
            setName("");
          }}
        >
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={`Save “${workspace?.name ?? "this workspace"}” layout as…`}
            className="h-8 flex-1 rounded-md border border-border bg-panel px-2.5 text-xs text-fg outline-none focus:border-accent"
          />
          <button
            disabled={!name.trim() || !workspace?.layout}
            className="rounded-md border border-border-strong px-2.5 py-1.5 text-xs text-fg hover:bg-hover disabled:opacity-40"
          >
            Save current
          </button>
        </form>
      </div>
    </div>
  );
}
