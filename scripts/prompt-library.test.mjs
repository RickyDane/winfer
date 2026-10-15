import { describe, expect, it } from "vitest";
import { findPrompt, normalizePrompts, parseVars, renderPrompt, usedBuiltins } from "../plugins/prompt-library/index.js";

describe("placeholders", () => {
  const body = "Write tests for {{file}} covering {{cases:errors}} in {{project}}. Again: {{ file }} {{cases}}";

  it("lists the ones to fill in, once each, with defaults", () => {
    expect(parseVars(body)).toEqual([
      { name: "file", def: "" },
      { name: "cases", def: "errors" },
    ]);
    expect(usedBuiltins(body)).toEqual(["project"]);
  });

  it("fills values, then defaults, else nothing", () => {
    expect(renderPrompt(body, { file: "a.ts", project: "/repo" })).toBe("Write tests for a.ts covering errors in /repo. Again: a.ts errors");
    expect(renderPrompt(body, { cases: "nulls" })).toBe("Write tests for  covering nulls in . Again:  nulls");
    expect(renderPrompt("no placeholders {single}", {})).toBe("no placeholders {single}");
  });
});

describe("normalizePrompts", () => {
  it("accepts a list or { prompts } and tidies tags", () => {
    const [p] = normalizePrompts({ prompts: [{ title: " Review ", body: "x", tags: ["Git", "git", " "] }] });
    expect(p).toMatchObject({ title: "Review", body: "x", tags: ["git"], uses: 0 });
    expect(p.id).toMatch(/^p/);
    expect(normalizePrompts([{ id: "keep", title: "a", body: "" }])[0].id).toBe("keep");
  });

  it("rejects bad input", () => {
    expect(() => normalizePrompts({})).toThrow(/list of prompts/);
    expect(() => normalizePrompts([{ title: "", body: "x" }])).toThrow(/Prompt 1/);
  });
});

describe("findPrompt", () => {
  const prompts = normalizePrompts([
    { id: "a", title: "Review my changes", body: "" },
    { id: "b", title: "Review", body: "" },
  ]);
  it("matches id, exact title, then a title's start", () => {
    expect(findPrompt(prompts, "a").id).toBe("a");
    expect(findPrompt(prompts, "review").id).toBe("b");
    expect(findPrompt(prompts, "review my").id).toBe("a");
    expect(findPrompt(prompts, "nope")).toBeNull();
  });
});
