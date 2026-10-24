import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

/** The harness's `planSidebar` setting (Settings → winfer code → Plan sidebar), shared by the TUI and the web UI. */
export const usePlanSidebar = create<{ on: boolean }>(() => ({ on: true }));

let loaded = false;

/** Reads the setting once from the harness config. */
export function loadPlanSidebar() {
  if (loaded) return;
  loaded = true;
  invoke<{ planSidebar?: boolean }>("harness_settings").then(
    (s) => usePlanSidebar.setState({ on: s.planSidebar !== false }),
    () => {},
  );
}

/** Turns the sidebar on/off and saves it, like the TUI's `/sidebar`. */
export async function setPlanSidebar(on: boolean) {
  usePlanSidebar.setState({ on });
  const settings = await invoke<Record<string, unknown>>("harness_settings");
  await invoke("harness_set_settings", { settings: { ...settings, planSidebar: on } });
}
