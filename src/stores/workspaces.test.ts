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

  it("drops console titles that are just a program's path", () => {
    expect(cleanAgentTitle("C:\\WINDOWS\\system32\\cmd.exe", [])).toBeNull();
    expect(cleanAgentTitle("Administrator: C:\\WINDOWS\\system32\\cmd.exe", [])).toBeNull();
    expect(cleanAgentTitle("C:\\Program Files\\PowerShell\\7\\pwsh.exe", [])).toBeNull();
    expect(cleanAgentTitle("/bin/zsh", [])).toBeNull();
    expect(cleanAgentTitle("Fix the /login redirect", [])).toBe("Fix the /login redirect");
  });
});
