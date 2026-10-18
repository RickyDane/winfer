import { describe, expect, it } from "vitest";
import { copyLabel, isOutdated, normalizeRepo, type InstalledSkill, type RepoListing, type SkillCopy } from "./skills";

describe("normalizeRepo", () => {
  it("accepts owner/repo and GitHub URLs", () => {
    expect(normalizeRepo("JuliusBrussee/caveman")).toBe("JuliusBrussee/caveman");
    expect(normalizeRepo(" https://github.com/anthropics/skills.git ")).toBe("anthropics/skills");
    expect(normalizeRepo("github.com/a/b/tree/main/skills")).toBe("a/b");
    expect(normalizeRepo("caveman")).toBeNull();
    expect(normalizeRepo("a b/c")).toBeNull();
  });
});

describe("isOutdated", () => {
  const skill = (hash: string, rev = "r1"): InstalledSkill => ({
    name: "caveman",
    description: "",
    copies: [{ target: "claude", label: "Claude Code", kind: "skill", project: null, dir: "/x", link: false, origin: { repo: "JuliusBrussee/caveman", path: "skills/caveman", rev, hash } }],
  });
  const listing = (hash: string): RepoListing => ({ repo: "JuliusBrussee/caveman", rev: "r2", skills: [{ name: "caveman", description: "", path: "skills/caveman", files: 1, hash }] });

  it("compares the skill folder's hash, not the repo's commit", () => {
    expect(isOutdated(skill("h1"), listing("h1"))).toBe(false);
    expect(isOutdated(skill("h1"), listing("h2"))).toBe(true);
  });
  it("falls back to the commit for installs without a hash", () => {
    expect(isOutdated(skill(""), listing("h1"))).toBe(true);
    expect(isOutdated(skill("", "r2"), listing("h1"))).toBe(false);
  });
  it("ignores skills from elsewhere", () => {
    expect(isOutdated({ name: "x", description: "", copies: [{ target: "claude", label: "Claude Code", kind: "skill", project: null, dir: "/x", link: true, origin: null }] }, listing("h"))).toBe(false);
    expect(isOutdated(skill("h1"), undefined)).toBe(false);
  });
});

describe("copyLabel", () => {
  const copy = (kind: SkillCopy["kind"], project: string | null): SkillCopy => ({ target: "cursor", label: "Cursor", kind, project, dir: "/x", link: false, origin: null });
  it("names the agent, commands and the project", () => {
    expect(copyLabel(copy("skill", null))).toBe("Cursor");
    expect(copyLabel(copy("command", "/Users/me/Coding/winfer/"))).toBe("Cursor command · winfer");
  });
});
