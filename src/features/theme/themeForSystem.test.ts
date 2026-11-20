import { describe, expect, it } from "vitest";
import { themeById, themeForSystem } from "./themes";

describe("themeForSystem", () => {
  it("uses the pick for the system's mode", () => {
    expect(themeForSystem(true, { dark: "github-dark", light: "github-light" }, "dark")).toBe("github-dark");
    expect(themeForSystem(false, { dark: "github-dark", light: "github-light" }, "dark")).toBe("github-light");
  });
  it("falls back to the current theme, or its counterpart", () => {
    expect(themeForSystem(true, {}, "dark")).toBe("dark");
    expect(themeForSystem(false, {}, "dark")).toBe(themeById("dark").pair);
  });
  it("ignores a pick that is not a theme of that mode", () => {
    expect(themeForSystem(true, { dark: "github-light" }, "dark")).toBe("dark");
    expect(themeForSystem(true, { dark: "nope" }, "dark")).toBe("dark");
  });
});
