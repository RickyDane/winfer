import { describe, expect, it } from "vitest";
import { replyLanguage, wrongScript } from "./llm";

describe("replyLanguage", () => {
  it("defaults terse tasks to English", () => {
    expect(replyLanguage("Release and push")).toBe("English");
  });
  it("detects German tasks", () => {
    expect(replyLanguage("Bitte die Tests für den Login fixen")).toBe("German");
  });
});

describe("wrongScript", () => {
  it("flags Cyrillic when the task has none", () => {
    expect(wrongScript("Начинаю задачу", "Release and push")).toBe(true);
  });
  it("accepts Latin text, accents and symbols", () => {
    expect(wrongScript("Prüfe Änderungen — 50% ✓", "Release and push")).toBe(false);
  });
  it("accepts Cyrillic when the task is Cyrillic", () => {
    expect(wrongScript("Начинаю задачу", "Сделай релиз")).toBe(false);
  });
});
