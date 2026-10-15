import { describe, expect, it } from "vitest";
import { parseChanges, splitArea } from "./releaseNotes";

const BODY = `### Changes
- Plugins: add Telegram, answer agents from your phone
- Settings: start agents in auto-approve mode

---
### Install
Unsigned builds.

- not a change
`;

describe("parseChanges", () => {
  it("reads the changes list and stops at the footer", () => {
    expect(parseChanges(BODY)).toEqual(["Plugins: add Telegram, answer agents from your phone", "Settings: start agents in auto-approve mode"]);
  });

  it("takes bullets without a heading, up to the rule", () => {
    expect(parseChanges("- one\n* two\n---\n- three")).toEqual(["one", "two"]);
  });

  it("handles empty notes", () => {
    expect(parseChanges(undefined)).toEqual([]);
    expect(parseChanges("")).toEqual([]);
  });
});

describe("splitArea", () => {
  it("splits the area prefix off", () => {
    expect(splitArea("Browser and side terminal: keep tabs")).toEqual({ area: "Browser and side terminal", text: "keep tabs" });
  });

  it("leaves plain subjects alone", () => {
    expect(splitArea("fix crash on start")).toEqual({ area: null, text: "fix crash on start" });
  });
});
