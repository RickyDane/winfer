import { describe, expect, it } from "vitest";
import { cleanAgentTitle } from "./workspaces";

describe("agent titles", () => {
  it("drops spinner glyphs and titles that only name the CLI", () => {
    const names = ["Claude Code", "claude"];
    expect(cleanAgentTitle("✳ Fix the login redirect", names)).toBe("Fix the login redirect");
    expect(cleanAgentTitle("⠂  Refactor   auth guard", names)).toBe("Refactor auth guard");
    expect(cleanAgentTitle("✳ Claude Code", names)).toBeNull();
    expect(cleanAgentTitle("  ", names)).toBeNull();
    expect(cleanAgentTitle("Über-Seite bauen", names)).toBe("Über-Seite bauen");
  });
});
