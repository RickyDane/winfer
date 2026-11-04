import { describe, expect, it } from "vitest";
import { skillCall } from "./commands";

describe("skillCall", () => {
  const skills = ["graphify", "model", "caveman"];

  it("parses a skill invocation with its args", () => {
    expect(skillCall("/graphify src/App.tsx", skills)).toEqual({ name: "graphify", args: "src/App.tsx" });
    expect(skillCall("/caveman", skills)).toEqual({ name: "caveman", args: "" });
  });

  it("lets built-in commands win over a skill of the same name", () => {
    expect(skillCall("/model opus", skills)).toBeNull();
  });

  it("ignores unknown commands and plain text", () => {
    expect(skillCall("/nope x", skills)).toBeNull();
    expect(skillCall("//graphify", skills)).toBeNull();
    expect(skillCall("graphify", skills)).toBeNull();
    expect(skillCall("/", skills)).toBeNull();
  });
});
