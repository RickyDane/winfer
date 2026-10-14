import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args: unknown) => invoke(cmd, args) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(), message: vi.fn(), open: vi.fn() }));
vi.mock("@/features/terminal/xtermRegistry", () => ({
  readTerminal: vi.fn(() => "Edited src/app.ts\n\n────────\nDone: added the login page."),
  closeTerminal: vi.fn(),
  sendText: vi.fn(),
  pasteText: vi.fn(),
}));

const { collectReview, diffStats, useReviews } = await import("./reviews");
const { useWorkspaces } = await import("@/stores/workspaces");

const DIFF = ["diff --git a/src/app.ts b/src/app.ts", "--- a/src/app.ts", "+++ b/src/app.ts", "+one", "+two", "-old"].join("\n");
const worktree = { repo: "/repo", path: "/wt/claude-1", branch: "winfer/claude-1", base: "main" };

function setPane(devUrl: string | null) {
  useWorkspaces.setState({
    workspaces: [
      {
        id: "w1",
        name: "repo",
        rootDir: "/repo",
        layout: { kind: "leaf", paneId: "p1" },
        panes: [{ id: "p1", workspaceId: "w1", provider: "claude", cwd: "/repo", worktree, devUrl, prompt: "Add a login page", raceId: null, status: "done" }],
      },
    ] as never,
    activeWorkspaceId: "w1",
    voiceConfig: { writer: "workers" },
    reviewConfig: {},
  });
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("review inbox", () => {
  beforeEach(() => {
    useReviews.setState({ reviews: [] });
    invoke.mockReset();
  });

  it("counts files, lines and untracked files from a worktree diff", () => {
    expect(diffStats({ diff: DIFF, untracked: ["src/new.ts"], ahead: 1, dirty: true })).toEqual({
      files: ["src/app.ts", "src/new.ts"],
      adds: 2,
      dels: 1,
      ahead: 1,
      dirty: true,
    });
  });

  it("skips turns that changed nothing", async () => {
    setPane(null);
    invoke.mockImplementation(async (cmd: string) => (cmd === "worktree_diff" ? { diff: "", untracked: [], ahead: 0, dirty: false } : null));
    await collectReview("p1");
    expect(useReviews.getState().reviews).toEqual([]);
  });

  it("builds one card per agent with summary, test command and screenshot", async () => {
    setPane("http://localhost:5173");
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "worktree_diff") return { diff: DIFF, untracked: [], ahead: 0, dirty: true };
      if (cmd === "llm_compose") return " Adds a login page. ";
      if (cmd === "review_test_command") return "pnpm test";
      if (cmd === "browser_screenshot") return "data:image/jpeg;base64,AAA";
      return null;
    });
    await collectReview("p1");
    await flush();
    await collectReview("p1"); // the agent finished another turn: same card, refreshed
    await flush();

    const reviews = useReviews.getState().reviews;
    expect(reviews).toHaveLength(1);
    const card = reviews[0];
    expect(card).toMatchObject({ branch: "winfer/claude-1", dir: "/wt/claude-1", testCommand: "pnpm test", prompt: "Add a login page" });
    expect(card.summary).toEqual({ state: "done", value: "Adds a login page." });
    expect(card.screenshot).toMatchObject({ state: "done", url: "http://localhost:5173" });
    expect(card.lastOutput).not.toContain("────");
    expect(card.tests).toEqual({ state: "idle" }); // tests only run automatically when enabled
    const summaryCall = invoke.mock.calls.find(([cmd]) => cmd === "llm_compose")![1] as { req: { user: string } };
    expect(summaryCall.req.user).toContain("Task: Add a login page");
    expect(summaryCall.req.user).toContain("+one");
  });
});
