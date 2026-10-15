import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args: unknown) => invoke(cmd, args) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(), message: vi.fn(), open: vi.fn() }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ readTerminal: vi.fn(() => ""), closeTerminal: vi.fn(), sendText: vi.fn(), pasteText: vi.fn() }));
const execute = vi.fn();
vi.mock("@/commands/bus", () => ({ execute: (cmd: unknown) => execute(cmd), mergePane: vi.fn(), discardPane: vi.fn() }));

const { enqueue, askAgentToResolve, useMergeQueue } = await import("./mergeQueue");
const { useReviews } = await import("./reviews");
const { conflictsFor } = await import("./conflicts");
const { useWorkspaces } = await import("@/stores/workspaces");

const wt = (n: number) => ({ repo: "/repo", path: `/wt/claude-${n}`, branch: `winfer/claude-${n}`, base: "main" });

function setup() {
  useWorkspaces.setState({
    workspaces: [
      {
        id: "w1",
        name: "repo",
        rootDir: "/repo",
        layout: null,
        panes: [1, 2, 3].map((n) => ({ id: `p${n}`, workspaceId: "w1", provider: "claude", cwd: "/repo", worktree: wt(n), status: "done", raceId: null })),
      },
    ] as never,
    activeWorkspaceId: "w1",
  });
  useReviews.setState({
    reviews: [1, 2, 3].map((n) => ({
      id: `r${n}`,
      paneId: `p${n}`,
      workspaceId: "w1",
      provider: "claude",
      raceId: null,
      prompt: `Task ${n}`,
      finishedAt: n,
      branch: `winfer/claude-${n}`,
      dir: `/wt/claude-${n}`,
      stats: { files: ["a.ts"], adds: 1, dels: 0, ahead: 1, dirty: false },
      lastOutput: "",
      summary: { state: "done", value: `Does thing ${n}.` },
      tests: { state: "idle" },
      testCommand: "pnpm test",
      screenshot: { state: "idle" },
      status: "open",
    })) as never,
  });
  useMergeQueue.setState({ items: [], running: false, runTests: true });
}

const settled = () => vi.waitFor(() => expect(useMergeQueue.getState().running).toBe(false));

describe("merge queue", () => {
  beforeEach(() => {
    invoke.mockReset();
    execute.mockReset();
    setup();
  });

  it("merges one branch at a time, syncing and testing each first, and keeps going past a stopped one", async () => {
    const calls: string[] = [];
    invoke.mockImplementation(async (cmd: string, args: { info?: { branch: string }; dir?: string }) => {
      const who = args?.info?.branch.slice(-1) ?? args?.dir?.slice(-1);
      calls.push(`${cmd}:${who}`);
      if (cmd === "worktree_sync") return who === "2" ? { kind: "conflicts", files: ["a.ts"] } : { kind: "synced", commits: 1 };
      if (cmd === "review_run_tests") return { command: "pnpm test", ok: who !== "3", output: "", ms: 10 };
      if (cmd === "worktree_diff") return { diff: "", untracked: [], ahead: 1, dirty: false };
      if (cmd === "usage_dirs") return [{ dir: `/wt/claude-${who}`, cost: 2.5, priced: true, tokens: 1000, calls: 3, firstAt: 1, lastAt: 2, models: [] }];
      return null;
    });

    enqueue(["r1", "r2", "r3"]);
    await settled();

    const states = Object.fromEntries(useMergeQueue.getState().items.map((i) => [i.reviewId, [i.state, i.files]]));
    expect(states).toEqual({ r1: ["merged", []], r2: ["conflict", ["a.ts"]], r3: ["failed", []] });
    // Branch 1 is fully done before branch 2 starts; nothing past the conflict for 2, nothing merged for 3.
    const steps = calls.filter((c) => /worktree_(commit|sync|merge)|review_run_tests/.test(c));
    expect(steps).toEqual([
      "worktree_commit:1",
      "worktree_sync:1",
      "review_run_tests:1",
      "worktree_merge:1",
      "worktree_commit:2",
      "worktree_sync:2",
      "worktree_commit:3",
      "worktree_sync:3",
      "review_run_tests:3",
    ]);
    expect(useReviews.getState().reviews.map((r) => r.status)).toEqual(["merged", "open", "open"]);
    // The merge is recorded with what it cost and the test run from the queue.
    const record = invoke.mock.calls.find(([cmd]) => cmd === "history_add")![1] as { merge: { cost: number; tests: string; branch: string } };
    expect(record.merge).toMatchObject({ branch: "winfer/claude-1", cost: 2.5, tests: "pnpm test passed" });
  });

  it("skips tests that already passed on an up-to-date branch, and when switched off", async () => {
    invoke.mockImplementation(async (cmd: string) => (cmd === "worktree_sync" ? { kind: "upToDate" } : cmd === "worktree_diff" ? { diff: "", untracked: [], ahead: 1, dirty: false } : null));
    useReviews.setState((s) => ({ reviews: s.reviews.map((r) => (r.id === "r1" ? { ...r, tests: { state: "done", value: { command: "pnpm test", ok: true, output: "", ms: 1 } } } : r)) }));
    enqueue(["r1"]);
    await settled();
    useMergeQueue.setState({ runTests: false });
    enqueue(["r2"]);
    await settled();
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "review_run_tests")).toEqual([]);
    expect(useMergeQueue.getState().items.map((i) => i.state)).toEqual(["merged", "merged"]);
  });

  it("hands a conflict back to the agent", async () => {
    invoke.mockImplementation(async (cmd: string) => (cmd === "worktree_sync" ? { kind: "conflicts", files: ["a.ts", "b.ts"] } : cmd === "worktree_diff" ? { diff: "", untracked: [], ahead: 1, dirty: false } : null));
    enqueue(["r1"]);
    await settled();
    askAgentToResolve("r1");
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ type: "pane.sendText", target: { id: "p1" }, submit: true }));
    expect((execute.mock.calls[0][0] as { text: string }).text).toContain("git merge main");
    expect((execute.mock.calls[0][0] as { text: string }).text).toContain("a.ts, b.ts");
    expect(useMergeQueue.getState().items).toEqual([]);
    expect(useReviews.getState().reviews.find((r) => r.id === "r1")).toBeUndefined();
  });

  it("doesn't queue a branch twice", () => {
    useMergeQueue.setState({ running: true }); // hold the runner
    enqueue(["r1", "r1"]);
    enqueue(["r1"]);
    expect(useMergeQueue.getState().items).toHaveLength(1);
  });
});

describe("conflict preview", () => {
  it("finds a worktree's own conflicts and the pairs it's in, either side", () => {
    const report = {
      items: [{ path: "/wt/claude-1", branch: "winfer/claude-1", files: ["a.ts"], behind: 2, baseConflicts: ["a.ts"] }],
      pairs: [
        { a: "/wt/claude-1", b: "/wt/claude-2", overlap: ["a.ts"], conflicts: [] },
        { a: "/wt/claude-3", b: "/wt/claude-1/", overlap: ["b.ts"], conflicts: ["b.ts"] },
      ],
    };
    const found = conflictsFor(report, "/wt/claude-1");
    expect(found.item?.baseConflicts).toEqual(["a.ts"]);
    expect(found.others.map((o) => o.other)).toEqual(["/wt/claude-2", "/wt/claude-3"]);
    expect(conflictsFor(report, "/wt/claude-9")).toEqual({ item: undefined, others: [] });
    expect(conflictsFor(null, "/wt/claude-1")).toEqual({ item: undefined, others: [] });
  });
});
