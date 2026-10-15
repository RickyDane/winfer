import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { create } from "zustand";
import { toast, updateToast } from "@/stores/toasts";
import { parseChanges, type ReleaseChanges, releasesBetween } from "./releaseNotes";

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
  /** Every release between this version and the available one, newest first. */
  availableNotes: ReleaseChanges[];
  /** Set when the last check failed (offline, or a release still being built). */
  error: string | null;
  /** The changes since the previous version, once updated, newest release first. */
  installed: { version: string; releases: ReleaseChanges[] } | null;
  /** Which release's changes the "What's new" dialog shows, if open. */
  notesOpen: "available" | "installed" | null;
}

export const useUpdates = create<UpdateState>(() => ({
  version: null,
  checking: false,
  installing: false,
  available: null,
  availableNotes: [],
  error: null,
  installed: null,
  notesOpen: null,
}));

export const showReleaseNotes = (which: "available" | "installed") => useUpdates.setState({ notesOpen: which });
export const closeReleaseNotes = () => useUpdates.setState({ notesOpen: null });

const REPO = "RickyDane/winfer";
/** The version this install last ran, to notice an update however it was installed. */
const LAST_VERSION = "winfer.lastVersion";
/** The changes of the release being installed, saved for the first launch after it. */
const PENDING_NOTES = "winfer.pendingNotes";

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
    const availableNotes = update ? await notesFor(update) : [];
    useUpdates.setState({ available: update, availableNotes });
    if (update && (!quiet || !offered.has(update.version))) {
      offered.add(update.version);
      const count = availableNotes.reduce((n, r) => n + r.changes.length, 0);
      toast(
        `winfer ${update.version} is available`,
        "info",
        count ? `${count} change${count === 1 ? "" : "s"}. Restarting stops the agents running in your panes.` : "Restarting stops the agents running in your panes.",
        count ? { label: "See what's new", run: () => showReleaseNotes("available") } : { label: "Install and restart", run: () => void installUpdate() },
      );
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
  const { available, availableNotes, installing } = useUpdates.getState();
  if (!available || installing) return;
  useUpdates.setState({ installing: true, notesOpen: null });
  const id = toast(`Downloading winfer ${available.version}…`, "loading");
  let total = 0;
  let received = 0;
  // Saved up front: on Windows the installer quits the app partway through downloadAndInstall.
  localStorage.setItem(PENDING_NOTES, JSON.stringify({ version: available.version, releases: availableNotes }));
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
  void getVersion().then(
    (version) => {
      useUpdates.setState({ version });
      void announceUpdate(version);
    },
    () => {},
  );
  if (import.meta.env.DEV) return;
  window.setTimeout(() => void checkForUpdates(true), FIRST_CHECK_MS);
  window.setInterval(() => void checkForUpdates(true), CHECK_EVERY_MS);
}

/** The changes of every release after `from` up to `to`, from GitHub. */
async function fetchChanges(from: string | null, to: string): Promise<ReleaseChanges[]> {
  const res = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=100`, { headers: { Accept: "application/vnd.github+json" } });
  if (!res.ok) throw new Error(`GitHub answered ${res.status}`);
  return releasesBetween(await res.json(), from, to);
}

/** What changed between this version and the update; just the update's own notes if GitHub can't say. */
async function notesFor(update: Update): Promise<ReleaseChanges[]> {
  const own = [{ version: update.version, changes: parseChanges(update.body) }];
  try {
    const releases = await fetchChanges(update.currentVersion, update.version);
    return releases.length ? releases : own;
  } catch (err) {
    console.warn("couldn't load release notes", err);
    return own;
  }
}

/** On the first launch of a new version, shows what changed in it. Fresh installs stay quiet. */
async function announceUpdate(version: string) {
  const last = localStorage.getItem(LAST_VERSION);
  const pending = localStorage.getItem(PENDING_NOTES);
  localStorage.setItem(LAST_VERSION, version);
  localStorage.removeItem(PENDING_NOTES);
  let releases: ReleaseChanges[] = [];
  try {
    // Older versions saved only the new release's changes.
    const saved = pending ? (JSON.parse(pending) as { version: string; releases?: ReleaseChanges[]; changes?: string[] }) : null;
    if (saved?.version === version) releases = saved.releases ?? [{ version, changes: saved.changes ?? [] }];
  } catch (err) {
    console.warn("couldn't read saved release notes", err);
  }
  // GitHub fills in releases the saved notes missed, e.g. when updating from before this list existed.
  if (last && last !== version && !import.meta.env.DEV) {
    const fetched = await fetchChanges(last, version).catch((err) => (console.warn("couldn't load release notes", err), []));
    if (fetched.length > releases.length) releases = fetched;
  }
  if (!releases.some((r) => r.changes.length)) return;
  useUpdates.setState({ installed: { version, releases }, notesOpen: "installed" });
}
