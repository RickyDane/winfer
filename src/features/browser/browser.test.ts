import { describe, expect, it } from "vitest";
import { normalizeUrl } from "./browser";

describe("normalizeUrl", () => {
  it.each([
    ["localhost:3000", "http://localhost:3000"],
    ["127.0.0.1:5173/app", "http://127.0.0.1:5173/app"],
    ["github.com", "https://github.com"],
    ["docs.rs/tokio", "https://docs.rs/tokio"],
    ["https://example.com/x", "https://example.com/x"],
    ["about:blank", "about:blank"],
    ["tauri multiwebview", "https://www.google.com/search?q=tauri%20multiwebview"],
  ])("%s → %s", (input, expected) => expect(normalizeUrl(input)).toBe(expected));
});
