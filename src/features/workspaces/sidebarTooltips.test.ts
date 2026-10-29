import { describe, expect, it } from "vitest";
import type { Pane } from "@/stores/workspaces";
import type { ExistingWorktree } from "@/features/worktree/worktree";
import { checkoutDetails, paneTooltip } from "./sidebarTooltips";

const main: ExistingWorktree = {
  info: { repo: "/repo", path: "/repo", branch: "release", base: "release" },
  isMain: true, prunable: false, byWinfer: false, dirty: false, ahead: 0,
  merged: false, createdAt: null, lastCommitAt: null,
};
const linked: ExistingWorktree = {
  ...main, isMain: false, dirty: true,
  info: { ...main.info, path: "/repo/.worktrees/task", branch: "winfer/current" },
};

describe("sidebar checkout details", () => {
  it("shows the actual checkout branch, keeping the full branch name", () => {
    expect(checkoutDetails("/repo/.worktrees/task/src", [main, linked])).toEqual([
      "Branch: winfer/current", "Worktree: /repo/.worktrees/task", "Base branch: release", "Changes: uncommitted changes",
    ]);
  });

  it("distinguishes the project checkout from an isolated worktree", () => {
    expect(checkoutDetails("/repo/src", [main, linked])).toEqual([
      "Branch: release", "Project checkout: /repo", "Changes: clean",
    ]);
  });

  it("matches normalized Windows paths and directory boundaries", () => {
    const wt = { ...linked, info: { ...linked.info, path: "C:/Repo/Task/" } };
    expect(checkoutDetails("c:\\repo\\task\\src", [wt])).toContain("Branch: winfer/current");
    expect(checkoutDetails("C:/Repo/Task-other", [wt])).toEqual(["Branch / worktree: unavailable for this directory"]);
  });

  it("does not invent a branch for loading, failed, missing, or non-Git checkouts", () => {
    expect(checkoutDetails("/repo", undefined)).toEqual(["Branch: checking…"]);
    expect(checkoutDetails("/repo", null)).toEqual(["Branch / worktree: unavailable"]);
    expect(checkoutDetails("/repo", [])).toEqual(["Git: not a repository"]);
    expect(checkoutDetails(linked.info.path, [{ ...linked, prunable: true }])).toEqual(["Branch / worktree: unavailable for this directory"]);
  });
});

describe("pane tooltip", () => {
  const pane = {
    cwd: linked.info.path, status: "needs-input", archived: true, model: "test-model",
    worktree: { ...linked.info, branch: "winfer/stale" }, wantsWorktree: false,
  } as Pane;

  it("includes session state and live branch rather than the saved branch", () => {
    const tooltip = paneTooltip(pane, "Codex", "Fix login", {
      task: null, title: "Fix login", percent: 70, step: "Waiting for approval", updatedAt: null, estimating: false, error: null,
    }, [main, linked]);
    expect(tooltip).toContain("Codex: Fix login");
    expect(tooltip).toContain("Status: Needs your input · archived");
    expect(tooltip).toContain("Model: test-model");
    expect(tooltip).toContain("Progress: 70%");
    expect(tooltip).toContain("Activity: Waiting for approval");
    expect(tooltip).toContain(`Directory: ${pane.cwd}`);
    expect(tooltip).toContain("Branch: winfer/current");
    expect(tooltip).not.toContain("winfer/stale");
  });

  it("reports pending worktree creation without inventing progress", () => {
    const tooltip = paneTooltip({ ...pane, wantsWorktree: true }, "Codex", "Codex", undefined, undefined);
    expect(tooltip).toContain("Worktree: creation pending");
    expect(tooltip).not.toContain("Progress:");
    expect(tooltip.split("\n")[0]).toBe("Codex");
  });
});
