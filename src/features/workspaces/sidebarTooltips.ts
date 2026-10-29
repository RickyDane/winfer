import { useRef, useState } from "react";
import type { Pane } from "@/stores/workspaces";
import type { Progress } from "@/features/progress/progress";
import { listWorktrees, type ExistingWorktree } from "@/features/worktree/worktree";

type CheckoutListing = ExistingWorktree[] | null | undefined;

/** Refresh on hover/focus, not on every render or for every session in the sidebar. */
export function useSidebarCheckouts() {
  const [listings, setListings] = useState<Record<string, CheckoutListing>>({});
  const pending = useRef(new Set<string>());
  const refresh = (dir: string) => {
    if (pending.current.has(dir)) return;
    pending.current.add(dir);
    setListings((s) => ({ ...s, [dir]: undefined }));
    void listWorktrees(dir)
      .then(
        (list) => setListings((s) => ({ ...s, [dir]: list })),
        () => setListings((s) => ({ ...s, [dir]: null })),
      )
      .finally(() => pending.current.delete(dir));
  };
  return { listings, refresh };
}

const normalize = (path: string) => {
  const value = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return /^[a-z]:\//i.test(value) ? value.toLowerCase() : value;
};

/** Match the directory, never a saved branch: the branch may have changed since spawn. */
export function checkoutDetails(dir: string, listing: CheckoutListing): string[] {
  if (listing === undefined) return ["Branch: checking…"];
  if (listing === null) return ["Branch / worktree: unavailable"];
  const path = normalize(dir);
  const checkout = listing
    .filter((wt) => !wt.prunable && (path === normalize(wt.info.path) || path.startsWith(`${normalize(wt.info.path)}/`)))
    .sort((a, b) => b.info.path.length - a.info.path.length)[0];
  if (!checkout) return [listing.length ? "Branch / worktree: unavailable for this directory" : "Git: not a repository"];
  return [
    `Branch: ${checkout.info.branch}`,
    `${checkout.isMain ? "Project checkout" : "Worktree"}: ${checkout.info.path}`,
    ...(!checkout.isMain ? [`Base branch: ${checkout.info.base}`] : []),
    `Changes: ${checkout.dirty ? "uncommitted changes" : "clean"}`,
  ];
}

const STATUS = { idle: "Idle", working: "Working", "needs-input": "Needs your input", done: "Done", error: "Error" };

export function paneTooltip(pane: Pane, name: string, label: string, progress: Progress | undefined, listing: CheckoutListing): string {
  return [
    label === name ? name : `${name}: ${label}`,
    `Status: ${STATUS[pane.status]}${pane.archived ? " · archived" : ""}`,
    ...(pane.model ? [`Model: ${pane.model}`] : []),
    ...(progress?.percent != null ? [`Progress: ${progress.percent}%`] : []),
    ...(progress?.step ? [`Activity: ${progress.step}`] : []),
    `Directory: ${pane.cwd}`,
    ...checkoutDetails(pane.cwd, listing),
    ...(pane.wantsWorktree ? ["Worktree: creation pending"] : []),
  ].join("\n");
}
