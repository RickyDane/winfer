/** Recipes: named agent setups (panes + layout + preview URL) you can load by key, palette or voice. */
import { PRESETS, buildPreset, leaves, type Preset } from "@/features/layout/tree";
import { openUrl, useBrowser } from "@/features/browser/browser";
import { logActivity } from "@/stores/activity";
import { providerName, useWorkspaces } from "@/stores/workspaces";
import { pluginRecipes } from "@/features/plugins/plugins";

export interface Recipe {
  id: string;
  name: string;
  panes: { provider: string; prompt?: string }[];
  preset: Preset;
  url?: string;
}

/** Shown until the user saves their own. */
export const BUILT_IN: Recipe[] = [
  { id: "builtin-pair", name: "Pair programming", panes: [{ provider: "claude" }, { provider: "codex" }], preset: "columns" },
  {
    id: "builtin-fullstack",
    name: "Full stack",
    panes: [{ provider: "claude" }, { provider: "codex" }, { provider: "shell" }],
    preset: "main-stack",
    url: "localhost:3000",
  },
];

/** The user's recipes (or the built-in ones until they save one), then plugins' recipes. */
export function allRecipes(): Recipe[] {
  const saved = useWorkspaces.getState().recipes;
  return [...(saved.length ? saved : BUILT_IN), ...pluginRecipes()];
}

export function describeRecipe(recipe: Recipe): string {
  const providers = useWorkspaces.getState().providers;
  return recipe.panes.map((p) => providerName(providers, p.provider)).join(" + ") + (recipe.url ? ` · ${recipe.url}` : "");
}

export function loadRecipe(id: string) {
  const recipe = allRecipes().find((r) => r.id === id);
  if (!recipe) return;
  const s = useWorkspaces.getState();
  const installed = new Set(s.providers.filter((p) => p.installed).map((p) => p.id));
  for (const pane of recipe.panes) {
    if (installed.has(pane.provider)) s.addPane(pane.provider, { prompt: pane.prompt });
  }
  s.updateActive((w) => ({
    layout: w.layout && buildPreset(recipe.preset, leaves(w.layout)),
    presetIndex: PRESETS.indexOf(recipe.preset),
    zoomedPaneId: null,
  }));
  if (recipe.url) void openUrl(recipe.url);
  logActivity(s.activeWorkspaceId, `Loaded recipe “${recipe.name}”`);
}

/** Saves the active workspace's panes, layout preset and current preview URL. */
export function saveCurrentAsRecipe(name: string) {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId);
  if (!ws?.layout) return;
  const tabs = useBrowser.getState().tabs[ws.id] ?? [];
  const activeTab = tabs.find((t) => t.id === useBrowser.getState().activeTab[ws.id]);
  const url = activeTab && activeTab.url !== "about:blank" ? activeTab.url : undefined;
  const recipe: Recipe = {
    id: `recipe-${crypto.randomUUID().slice(0, 8)}`,
    name,
    panes: leaves(ws.layout).map((id) => ({ provider: ws.panes.find((p) => p.id === id)!.provider })),
    preset: PRESETS[ws.presetIndex] ?? "auto",
    url,
  };
  s.setRecipes([...s.recipes, recipe]);
}

export function deleteRecipe(id: string) {
  const s = useWorkspaces.getState();
  s.setRecipes(s.recipes.filter((r) => r.id !== id));
}
