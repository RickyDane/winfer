import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useWorkspaces } from "@/stores/workspaces";
import { toast, updateToast } from "@/stores/toasts";
import type { ProviderId } from "./providers";

/** Mirrors `VersionStatus` in src-tauri/src/provider_updates.rs. */
export interface ProviderVersion {
  id: ProviderId;
  current: string | null;
  /** Null when there's no way to tell (offline, or the agent names no source). */
  latest: string | null;
  outdated: boolean;
  install: { kind: "brew"; name: string; cask: boolean } | { kind: "package"; manager: string } | { kind: "own" };
  /** Whether winfer knows a command that updates it. */
  updatable: boolean;
}

/**
 * Newer versions of the agent CLIs. Checked in the background like winfer's own updates;
 * updating only changes the CLI on disk, so running panes keep their version until restarted.
 */
interface ProviderUpdateState {
  versions: Record<ProviderId, ProviderVersion>;
  checking: boolean;
  /** Agents being updated right now. */
  updating: ProviderId[];
  checkedAt: number | null;
}

export const useProviderUpdates = create<ProviderUpdateState>(() => ({ versions: {}, checking: false, updating: [], checkedAt: null }));

const FIRST_CHECK_MS = 30_000;
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

/** "id@version" already offered in a toast, so the periodic check doesn't nag about the same one. */
const offered = new Set<string>();

const nameOf = (id: ProviderId) => useWorkspaces.getState().providers.find((p) => p.id === id)?.name ?? id;

/** How the agent gets updated, for the settings row. */
export function installLabel(v: ProviderVersion): string {
  if (v.install.kind === "brew") return "Homebrew";
  if (v.install.kind === "package") return v.install.manager;
  return "its own updater";
}

/** Checks every installed agent; `quiet` (background checks) only speaks up about new versions. */
export async function checkProviderUpdates(quiet = false): Promise<ProviderVersion[]> {
  if (useProviderUpdates.getState().checking) return Object.values(useProviderUpdates.getState().versions);
  useProviderUpdates.setState({ checking: true });
  try {
    const list = await invoke<ProviderVersion[]>("providers_versions");
    useProviderUpdates.setState({ versions: Object.fromEntries(list.map((v) => [v.id, v])), checkedAt: Date.now() });
    const fresh = list.filter((v) => v.outdated && v.updatable && (!quiet || !offered.has(`${v.id}@${v.latest}`)));
    fresh.forEach((v) => offered.add(`${v.id}@${v.latest}`));
    if (fresh.length === 1) {
      const [v] = fresh;
      toast(`${nameOf(v.id)} ${v.latest} is available`, "info", `You have ${v.current}.`, { label: "Update", run: () => void updateProvider(v.id) });
    } else if (fresh.length > 1) {
      toast(`${fresh.length} agent updates available`, "info", fresh.map((v) => `${nameOf(v.id)} ${v.latest}`).join(", "), {
        label: "Update all",
        run: () => fresh.forEach((v) => void updateProvider(v.id)),
      });
    } else if (!quiet) {
      toast(list.some((v) => v.outdated) ? "Some agents can only be updated by hand" : "Your agents are up to date", "success");
    }
    return list;
  } catch (err) {
    if (!quiet) toast("Couldn't check for agent updates", "error", String(err));
    return [];
  } finally {
    useProviderUpdates.setState({ checking: false });
  }
}

/** Installs the newest version of one agent CLI. */
export async function updateProvider(id: ProviderId) {
  if (useProviderUpdates.getState().updating.includes(id)) return;
  useProviderUpdates.setState((s) => ({ updating: [...s.updating, id] }));
  const name = nameOf(id);
  const toastId = toast(`Updating ${name}…`, "loading");
  try {
    const v = await invoke<ProviderVersion>("providers_update", { id });
    useProviderUpdates.setState((s) => ({ versions: { ...s.versions, [id]: v } }));
    updateToast(toastId, {
      kind: "success",
      text: v.current ? `${name} is now ${v.current}` : `${name} updated`,
      detail: "New panes use it; running ones keep the old version until restarted.",
    });
  } catch (err) {
    updateToast(toastId, { kind: "error", text: `Couldn't update ${name}`, detail: String(err) });
  } finally {
    useProviderUpdates.setState((s) => ({ updating: s.updating.filter((x) => x !== id) }));
  }
}

let started = false;

/** Background checks: a little after launch (once the panes are up), then every few hours. */
export function startProviderUpdateChecks() {
  if (started) return;
  started = true;
  window.setTimeout(() => void checkProviderUpdates(true), FIRST_CHECK_MS);
  window.setInterval(() => void checkProviderUpdates(true), CHECK_EVERY_MS);
}
