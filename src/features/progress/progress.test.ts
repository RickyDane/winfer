import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
let output = "";
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args: unknown) => invoke(cmd, args) }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ readTerminal: vi.fn(() => output), outputAge: vi.fn(() => 0) }));

const { cleanTitle, estimate, markFinished, nameSession, parseEstimate, setTask, taskHeadline, useProgress } = await import("./progress");
const { useWorkspaces } = await import("@/stores/workspaces");
const { useChatGpt } = await import("@/features/llm/chatgpt");
// The LLM runs on the ChatGPT sign-in.
useChatGpt.setState({ signedIn: true });

const pane = { id: "p1", workspaceId: "w1", provider: "claude", cwd: "/repo", worktree: null, prompt: "Build a Flappy Bird clone", status: "working" } as never;

describe("session progress", () => {
  beforeEach(() => {
    useProgress.setState({ byPane: {} });
    useWorkspaces.setState({ voiceConfig: { writer: "chatgpt" } });
    invoke.mockReset();
    output = "";
  });

  it("saves the task and title so session titles survive restarts", () => {
    useWorkspaces.setState({ paneTasks: {} });
    setTask("p1", "Build a Flappy Bird clone");
    expect(useWorkspaces.getState().paneTasks.p1).toEqual({ task: "Build a Flappy Bird clone", title: null });
  });

  it("parses estimates with prose around the JSON and clamps them", () => {
    expect(parseEstimate('Sure: {"percent": 45.4, "step": "Writing game loop", "title": "Flappy Bird clone"}')).toEqual({
      percent: 45,
      step: "Writing game loop",
      title: "Flappy Bird clone",
    });
    expect(parseEstimate('{"percent": 140}')).toMatchObject({ percent: 100, step: null });
    expect(parseEstimate("no idea")).toBeNull();
    expect(parseEstimate('{"percent": "lots"}')).toBeNull();
  });

  it("estimates from the task and output, only moves forward, and skips unchanged output", async () => {
    output = "☑ Create index.html\n☐ Add pipes\nWriting game.js…";
    invoke.mockResolvedValueOnce('{"percent": 40, "step": "Writing game.js", "title": "Flappy Bird"}');
    await estimate(pane);
    expect(useProgress.getState().byPane.p1).toMatchObject({ percent: 40, step: "Writing game.js", title: "Flappy Bird" });
    const user = (invoke.mock.calls[0][1] as { req: { user: string } }).req.user;
    expect(user).toContain("Task: Build a Flappy Bird clone");
    expect(user).toContain("☐ Add pipes");

    await estimate(pane); // same output: no new LLM call
    expect(invoke).toHaveBeenCalledTimes(1);

    output += "\nAdding pipes";
    invoke.mockResolvedValueOnce('{"percent": 25, "step": "Adding pipes"}'); // a noisy lower guess
    await estimate(pane);
    expect(useProgress.getState().byPane.p1.percent).toBe(40);

    markFinished("p1");
    expect(useProgress.getState().byPane.p1.percent).toBe(100);
  });

  it("starts over on a new task but ignores short replies", () => {
    useProgress.setState({ byPane: { p1: { task: "old", title: "Old", percent: 80, step: "x", updatedAt: 1, estimating: false, error: null } } });
    setTask("p1", "y");
    expect(useProgress.getState().byPane.p1.percent).toBe(80);
    setTask("p1", "Now add a high score table");
    expect(useProgress.getState().byPane.p1).toMatchObject({ task: "Now add a high score table", percent: 0, title: "Old" });
  });

  it("names the session with the LLM, keeping the current title as context", async () => {
    useProgress.setState({ byPane: { p1: { task: "x", title: "Flappy Bird clone", percent: 0, step: null, updatedAt: null, estimating: false, error: null } } });
    invoke.mockResolvedValueOnce('Title: "Add high score table."');
    await nameSession("p1", "Now add a high score table to the game");
    expect(useProgress.getState().byPane.p1.title).toBe("Add high score table");
    expect((invoke.mock.calls[0][1] as { req: { user: string } }).req.user).toContain("Current title: Flappy Bird clone");
  });

  it("cleans title answers and rejects prose", () => {
    expect(cleanTitle("**Fix login redirect**\n")).toBe("Fix login redirect");
    expect(cleanTitle("Sure, here is a title that describes what the agent is doing in this session right now")).toBeNull();
  });

  it("falls back to a trimmed headline of the task", () => {
    expect(taskHeadline("I noticed that the session titles are lost when restarting. Please fix that")).toBe("The session titles are lost when restarting");
    expect(taskHeadline("Also: please add dark mode to the settings page and all the dialogs in the app")).toBe("Add dark mode to the settings page and…");
  });

  it("doesn't estimate an agent that has no task yet (just started)", async () => {
    output = "OpenCode v1.0 — ask anything";
    await estimate(Object.assign({}, pane, { prompt: null }));
    expect(invoke).not.toHaveBeenCalled();
    expect(useProgress.getState().byPane.p1).toBeUndefined();
  });

  it("keeps a working agent below 100 but lets a stopped one finish", async () => {
    const setStatus = (status: string) =>
      useWorkspaces.setState({ workspaces: [{ id: "w1", panes: [Object.assign({}, pane, { status })] }] as never });
    setStatus("working");
    output = "all tests pass";
    invoke.mockResolvedValueOnce('{"percent": 100, "step": "Done"}');
    await estimate(pane);
    expect(useProgress.getState().byPane.p1.percent).toBe(99);

    setStatus("idle");
    output += "\n> ";
    invoke.mockResolvedValueOnce('{"percent": 100, "step": "Done"}');
    await estimate(pane);
    expect(useProgress.getState().byPane.p1.percent).toBe(100);
    expect((invoke.mock.calls[1][1] as { req: { user: string } }).req.user).toContain("Agent state: stopped");
  });

  it("doesn't call the LLM with the prompt writer off", async () => {
    useWorkspaces.setState({ voiceConfig: { writer: "off" } });
    output = "working";
    await estimate(pane);
    expect(invoke).not.toHaveBeenCalled();
  });
});
