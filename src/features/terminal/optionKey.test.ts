import { describe, expect, it } from "vitest";
import { optionTypedChar } from "./optionKey";

const key = (key: string, code: string, extra: Partial<KeyboardEvent> = {}) =>
  ({ type: "keydown", key, code, altKey: true, metaKey: false, ctrlKey: false, ...extra }) as KeyboardEvent;

describe("optionTypedChar", () => {
  it("types characters that layouts put on Option", () => {
    expect(optionTypedChar(key("@", "KeyL"))).toBe("@"); // German
    expect(optionTypedChar(key("[", "Digit5"))).toBe("[");
    expect(optionTypedChar(key("{", "Digit8"))).toBe("{");
    expect(optionTypedChar(key("|", "Digit7"))).toBe("|");
    expect(optionTypedChar(key("~", "KeyN"))).toBe("~");
  });

  it("keeps Option as Meta otherwise", () => {
    expect(optionTypedChar(key("¬", "KeyL"))).toBeNull(); // US Option+L
    expect(optionTypedChar(key("∫", "KeyB"))).toBeNull();
    expect(optionTypedChar(key("b", "KeyB"))).toBeNull();
    expect(optionTypedChar(key("Backspace", "Backspace"))).toBeNull();
    expect(optionTypedChar(key("@", "KeyL", { altKey: false }))).toBeNull();
    expect(optionTypedChar(key("@", "KeyL", { ctrlKey: true }))).toBeNull();
    expect(optionTypedChar(key("@", "KeyL", { type: "keyup" }))).toBeNull();
  });
});
