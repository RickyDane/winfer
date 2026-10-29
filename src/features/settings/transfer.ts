import { invoke } from "@tauri-apps/api/core";

/** What an export file says about itself (`transfer::Manifest`). */
export interface ExportManifest {
  format: number;
  appVersion: string;
  createdAt: number;
  home: string;
  platform: string;
  hasSecrets: boolean;
}

/** Keys that describe this install rather than the user's setup. */
const LOCAL_ONLY = new Set(["winfer-mock-state", "winfer.lastVersion", "winfer.pendingNotes"]);

function localStorageEntries(): Record<string, string> {
  const out: Record<string, string> = {};
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i);
    if (key !== null && !LOCAL_ONLY.has(key)) out[key] = localStorage.getItem(key) ?? "";
  }
  return out;
}

export function exportData(path: string, passphrase: string | null): Promise<void> {
  return invoke("data_export", { path, localStorage: localStorageEntries(), passphrase });
}

export function inspectExport(path: string): Promise<ExportManifest> {
  return invoke("data_import_inspect", { path });
}

/** Unpacks the export for the next start; the app must relaunch to apply it. */
export function stageImport(path: string, passphrase: string | null): Promise<void> {
  return invoke("data_import_stage", { path, passphrase });
}

export async function restoreImportedLocalStorage() {
  try {
    const entries = await invoke<Record<string, string> | null>("data_import_take_local");
    if (!entries) return;
    for (const [key, value] of Object.entries(entries)) localStorage.setItem(key, value);
  } catch (err) {
    console.error("restoring imported localStorage failed", err);
  }
}
