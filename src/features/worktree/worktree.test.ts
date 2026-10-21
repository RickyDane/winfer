import { describe, expect, it } from "vitest";
import { worktreeInUse } from "./worktree";

const wt = { repo: "/private/repo", path: "/private/wts/feat", branch: "winfer/feat", base: "main" };
const pane = (over: object) => ({ workspaceId: "w1", cwd: "/repo", worktree: null, ...over });

describe("worktreeInUse", () => {
  it("matches the same path, also with other slashes or a trailing slash", () => {
    expect(worktreeInUse(wt, [pane({ worktree: { ...wt, path: "/private/wts/feat/" } })], "w1")).toBe(true);
    const win = { ...wt, path: "C:/Users/a/wts/feat" };
    expect(worktreeInUse(win, [pane({ worktree: { ...win, path: "c:\\Users\\a\\wts\\feat" } })], "w1")).toBe(true);
  });

  it("matches the branch when git reports the real path", () => {
    const p = pane({ worktree: { repo: "/repo", path: "/wts/feat", branch: "winfer/feat", base: "main" } });
    expect(worktreeInUse(wt, [p], "w1")).toBe(true);
    // Same branch name in another workspace's repo is someone else's.
    expect(worktreeInUse(wt, [{ ...p, workspaceId: "w2" }], "w1")).toBe(false);
  });

  it("counts panes that only have their folder in the worktree", () => {
    expect(worktreeInUse(wt, [pane({ cwd: "/private/wts/feat/src" })], "w1")).toBe(true);
    expect(worktreeInUse(wt, [pane({ cwd: "/private/wts/feat-2" })], "w1")).toBe(false);
  });

  it("is free when no pane is there", () => {
    expect(worktreeInUse(wt, [pane({})], "w1")).toBe(false);
  });
});
