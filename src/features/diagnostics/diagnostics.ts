import { invoke } from "@tauri-apps/api/core";

export interface MemorySnapshot {
  rssBytes: number | null;
  peakRssBytes: number | null;
  systemTotalBytes: number | null;
  systemAvailableBytes: number | null;
}

export interface DiagnosticsInfo {
  version: string;
  os: string;
  arch: string;
  logDir: string;
  logBytes: number;
  crashBytes: number;
  dumpCount: number;
  memory: MemorySnapshot;
  sessionPeakRssBytes: number;
  uptimeSecs: number;
  ptySessions: number;
  scrollbackBytes: number;
  previousUnexpectedExit: boolean;
  verbose: boolean;
  crashTail: string;
  recentMemory: string[];
}

export const getDiagnostics = () => invoke<DiagnosticsInfo>("diagnostics_info");
export const setVerboseLogging = (on: boolean) => invoke<void>("diagnostics_set_verbose", { on });
export const exportDiagnostics = (path: string) => invoke<string>("diagnostics_export", { path });
export const clearDiagnostics = () => invoke<void>("diagnostics_clear");

export const mb = (bytes: number | null | undefined) =>
  bytes == null ? "?" : `${(bytes / 1048576).toFixed(bytes >= 1073741824 ? 2 : 0)} MB`;
