import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ readTerminal: vi.fn(() => "") }));

const { parseMessages } = await import("./writer");

describe("parseMessages", () => {
  it("takes the JSON array, with or without surrounding text", () => {
    expect(parseMessages('Here you go:\n["Build Flappy Bird.", "Build Super Mario."]', 2)).toEqual(["Build Flappy Bird.", "Build Super Mario."]);
  });

  it("rejects plans that don't match the recipients", () => {
    expect(parseMessages('["only one"]', 2)).toBeNull();
    expect(parseMessages("no json here", 1)).toBeNull();
    expect(parseMessages('["a", ""]', 2)).toBeNull();
  });

  it("takes any non-empty list when no count is expected", () => {
    expect(parseMessages('["Create credentials.js.", "Add decide()."]')).toHaveLength(2);
    expect(parseMessages("[]")).toBeNull();
  });
});
