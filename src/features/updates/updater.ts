import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { create } from "zustand";
import { toast, updateToast } from "@/stores/toasts";

/**
 * App updates from GitHub releases: the updater plugin reads latest.json on the latest release
 * (built by release.yml), and installs the signed bundle for this platform. Nothing installs
 * without the user's say-so, since restarting stops every running agent.
 */
interface UpdateState {
  version: string | null;
  checking: boolean;
  installing: boolean;
  /** The newer release, once a check found one. */
  available: Update | null;
  /** Set when the last check failed (offline, or a release still being built). */
  error: string | null;
}

export const useUpdates = create<UpdateState>(() => ({ version: null, checking: false, installing: false, available: null, error: null }));

const FIRST_CHECK_MS = 10_000;
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

/** Versions already offered in a toast, so the periodic check doesn't nag about the same one. */
const offered = new Set<string>();

/** Checks now; `quiet` (background checks) skips the "up to date" and error toasts. */
export async function checkForUpdates(quiet = false): Promise<Update | null> {
  if (useUpdates.getState().checking) return useUpdates.getState().available;
  useUpdates.setState({ checking: true, error: null });
  try {
    const update = await check();
    useUpdates.setState({ available: update });
    if (update && (!quiet || !offered.has(update.version))) {
      offered.add(update.version);
      toast(`winfer ${update.version} is available`, "info", "Restarting stops the agents running in your panes.", {
        label: "Install and restart",
        run: () => void installUpdate(),
      });
    } else if (!update && !quiet) {
      toast("winfer is up to date", "success");
    }
    return update;
  } catch (err) {
    useUpdates.setState({ error: String(err) });
    if (!quiet) toast("Couldn't check for updates", "error", String(err));
    return null;
  } finally {
    useUpdates.setState({ checking: false });
  }
}

/** Downloads and installs the available update, then relaunches into it. */
export async function installUpdate() {
  const { available, installing } = useUpdates.getState();
  if (!available || installing) return;
  useUpdates.setState({ installing: true });
  const id = toast(`Downloading winfer ${available.version}…`, "loading");
  let total = 0;
  let received = 0;
  try {
    await available.downloadAndInstall((event) => {
      if (event.event === "Started") total = event.data.contentLength ?? 0;
      else if (event.event === "Progress") {
        received += event.data.chunkLength;
        if (total) updateToast(id, { text: `Downloading winfer ${available.version}… ${Math.round((received / total) * 100)}%` });
      } else if (event.event === "Finished") updateToast(id, { text: `Installing winfer ${available.version}…` });
    });
    // Windows quits into the installer by itself; macOS and Linux need the relaunch.
    await relaunch();
  } catch (err) {
    updateToast(id, { kind: "error", text: "Update failed", detail: String(err) });
    useUpdates.setState({ installing: false });
  }
}

let started = false;

/** Background checks: shortly after launch, then every few hours. Release builds only. */
export function startUpdateChecks() {
  if (started) return;
  started = true;
  void getVersion().then((version) => useUpdates.setState({ version }), () => {});
  if (import.meta.env.DEV) return;
  window.setTimeout(() => void checkForUpdates(true), FIRST_CHECK_MS);
  window.setInterval(() => void checkForUpdates(true), CHECK_EVERY_MS);
}
