import { describe, expect, it } from "vitest";
import { LIVE_ROLE, liveInstructions } from "./instructions";

const ctx = {
  providers: [],
  workspaces: [],
  active: { id: "ws", name: "api", rootDir: "~/api", panes: [], layout: null },
  focusedPaneId: null,
  browserTabs: [],
  lastCommands: [],
  models: {},
  recentAddresses: {},
} as never;

describe("live voice instructions", () => {
  it("uses the default prompt unless the user set one, then adds winfer's state", () => {
    expect(liveInstructions(ctx)).toMatch(new RegExp(`^${LIVE_ROLE.slice(0, 40)}`));
    expect(liveInstructions(ctx, "   ")).toContain(LIVE_ROLE);
    const custom = liveInstructions(ctx, "Talk like a pirate.");
    expect(custom.startsWith("Talk like a pirate.\n\n")).toBe(true);
    expect(custom).not.toContain(LIVE_ROLE);
    expect(custom).toContain("Current workspace: api (~/api)");
  });
});
