import { describe, expect, it } from "vitest";
import { compareVersions, parseChanges, releasesBetween, splitArea } from "./releaseNotes";

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

describe("compareVersions", () => {
  it("compares numerically", () => {
    expect(compareVersions("0.1.9", "0.1.10")).toBeLessThan(0);
    expect(compareVersions("v0.2.0", "0.1.43")).toBeGreaterThan(0);
    expect(compareVersions("0.1.43", "v0.1.43")).toBe(0);
  });

  it("orders nebula builds before their release", () => {
    expect(compareVersions("0.1.102-7", "0.1.102")).toBeLessThan(0);
    expect(compareVersions("0.1.102-7", "0.1.101")).toBeGreaterThan(0);
    expect(compareVersions("0.1.102-9", "0.1.102-10")).toBeLessThan(0);
  });
});

describe("releasesBetween", () => {
  const releases = [
    { tag_name: "v0.1.44", body: "- four" },
    { tag_name: "v0.1.43", body: "- three" },
    { tag_name: "v0.1.42", body: "- two" },
    { tag_name: "v0.1.41", body: "- one" },
    { tag_name: "v0.1.45", body: "- draft", draft: true },
  ];

  it("lists every release after the current one, newest first", () => {
    expect(releasesBetween(releases, "0.1.41", "0.1.43")).toEqual([
      { version: "0.1.43", changes: ["three"] },
      { version: "0.1.42", changes: ["two"] },
    ]);
  });

  it("takes only the target without a starting version", () => {
    expect(releasesBetween(releases, null, "0.1.44")).toEqual([{ version: "0.1.44", changes: ["four"] }]);
  });

  it("includes nebula builds only when updating to one", () => {
    const withNebula = [...releases, { tag_name: "v0.1.45-3", body: "- u3", prerelease: true }, { tag_name: "v0.1.45-2", body: "- u2", prerelease: true }];
    expect(releasesBetween(withNebula, "0.1.44", "0.1.45-3")).toEqual([
      { version: "0.1.45-3", changes: ["u3"] },
      { version: "0.1.45-2", changes: ["u2"] },
    ]);
    expect(releasesBetween(withNebula, "0.1.43", "0.1.44")).toEqual([{ version: "0.1.44", changes: ["four"] }]);
  });
});
