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
  });
});
