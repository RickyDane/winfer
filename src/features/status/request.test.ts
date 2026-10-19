import { describe, expect, it } from "vitest";
import { describeRequest } from "./request";

describe("describeRequest", () => {
  it("reads Claude Code's permission prompt", () => {
    const screen = [
      "⏺ I'll remove the build folder.",
      "╭────────────────────────────────────────╮",
      "│ Bash command                            │",
      "│   rm -rf dist                           │",
      "│   Remove the old build output           │",
      "│                                         │",
      "│ Do you want to proceed?                 │",
      "│ ❯ 1. Yes                                │",
      "│   2. Yes, and don't ask again           │",
      "│   3. No, and tell Claude what to do     │",
      "╰────────────────────────────────────────╯",
    ].join("\n");
    const r = describeRequest(screen, "Claude needs your permission to use Bash");
    expect(r.kind).toBe("approval");
    expect(r.text).toBe("Bash command\nrm -rf dist\nRemove the old build output\nDo you want to proceed?");
    expect(r.options?.map((o) => o.label)).toEqual(["Yes", "Yes, and don't ask again", "No, and tell Claude what to do"]);
  });

  it("reads a question's options with their descriptions (Claude Code's AskUserQuestion)", () => {
    const screen = [
      "☐ Library",
      "",
      "Which state library should the store use?",
      "",
      "❯ 1. Zustand",
      "     Small, hook-based, already used in src/",
      "  2. Redux Toolkit",
      "     More structure, more boilerplate",
      "  3. Type something.",
      "",
      "Enter to select · ↑/↓ to navigate · Esc to cancel",
    ].join("\n");
    const r = describeRequest(screen, "Claude is waiting for your input");
    expect(r.kind).toBe("question");
    expect(r.text).toContain("Which state library should the store use?");
    expect(r.options).toEqual([
      { n: 1, label: "Zustand", detail: "Small, hook-based, already used in src/" },
      { n: 2, label: "Redux Toolkit", detail: "More structure, more boilerplate" },
      { n: 3, label: "Type something." },
    ]);
  });

  it("takes the line above a picker as its prompt when it has no question mark", () => {
    const r = describeRequest("Pick a deploy target\n❯ 1. staging\n  2. production\n");
    expect(r.text).toBe("Pick a deploy target");
    expect(r.options?.map((o) => o.n)).toEqual([1, 2]);
  });

  it("treats an open question as a question", () => {
    const screen = "I found two config files.\nWhich one should I use for production, config.prod.ts or settings.ts?\n> ";
    const r = describeRequest(screen);
    expect(r.kind).toBe("question");
    expect(r.text).toContain("Which one should I use for production");
  });

  it("falls back to the hook's message when the screen has no question", () => {
    const r = describeRequest("✻ Thinking…\n", "Claude is waiting for your input");
    expect(r).toEqual({ kind: "question", text: "Claude is waiting for your input" });
  });

  it("recognises y/n prompts", () => {
    expect(describeRequest("Overwrite package.json? (y/n)").kind).toBe("approval");
    expect(describeRequest("Overwrite package.json? (y/n)").options).toBeUndefined();
  });
});
