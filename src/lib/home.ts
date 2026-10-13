import { homeDir } from "@tauri-apps/api/path";

let home = "";

/** The user's home folder, for resolving "~" in paths agents print. Loaded once at startup. */
export async function loadHome() {
  home = (await homeDir().catch(() => "")).replace(/\/$/, "");
}

export const getHome = () => home;
