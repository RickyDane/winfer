import { describe, expect, it } from "vitest";
import type { Pane } from "../protocol";
import { matchesPane } from "./paneSearch";

const pane: Pane = { id: "p", title: "Fix login", workspace: "relay", provider: "claude", status: "working", message: "Running tests", updatedAt: 0, request: null, reply: null };

describe("matchesPane", () => {
  it("finds workspaces, sessions and agents, ignoring case", () => {
    for (const q of ["", "  ", "RELAY", "login", "claude code", "tests"]) expect(matchesPane(pane, q, "Claude Code")).toBe(true);
    expect(matchesPane(pane, "billing")).toBe(false);
  });

  it("needs every word, from any field or the loaded thread", () => {
    expect(matchesPane(pane, "relay login")).toBe(true);
    expect(matchesPane(pane, "relay billing")).toBe(false);
    expect(matchesPane(pane, "relay oauth", "Claude Code", [{ id: "m", role: "user", text: "use OAuth", ts: null }])).toBe(true);
  });
});
