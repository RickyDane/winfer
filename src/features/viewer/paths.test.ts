import { describe, expect, it } from "vitest";
import { fileFromUri, findPaths, relativeTo, slugger } from "./paths";

const paths = (text: string) => findPaths(text).map((m) => (m.line ? `${m.path}:${m.line}` : m.path));

describe("findPaths", () => {
  it("finds the paths agents print", () => {
    expect(paths("⏺ Read(docs/PLAN.md)")).toEqual(["docs/PLAN.md"]);
    expect(paths("• Edited src/features/viewer/paths.ts (+3 -1)")).toEqual(["src/features/viewer/paths.ts"]);
    expect(paths("← Edit ./README.md")).toEqual(["./README.md"]);
    expect(paths("see `~/notes/todo.md` and /Users/r/x/CHANGELOG.md.")).toEqual(["~/notes/todo.md", "/Users/r/x/CHANGELOG.md"]);
    expect(paths("error at src/app/AppShell.tsx:42:7")).toEqual(["src/app/AppShell.tsx:42"]);
    expect(paths("C:\\work\\repo\\README.md")).toEqual(["C:\\work\\repo\\README.md"]);
    expect(paths("→ Read src/foo.ts:10-20")).toEqual(["src/foo.ts:10"]);
    expect(paths("see `src/app.ts#L42`")).toEqual(["src/app.ts:42"]);
  });

  it("reports where the match is, suffix included", () => {
    const [m] = findPaths("at src/a.ts:12 now");
    expect([m.start, m.end]).toEqual([3, 14]);
  });

  it("skips versions, URLs and plain words", () => {
    expect(paths("v1.2.3 and 0.1.85")).toEqual([]);
    expect(paths("open https://github.com/x/y/blob/main/README.md")).toEqual([]);
    expect(paths("hello world")).toEqual([]);
  });
});

describe("fileFromUri", () => {
  it("reads file and editor links", () => {
    expect(fileFromUri("file:///Users/r/My%20Notes/a.md")).toEqual({ path: "/Users/r/My Notes/a.md", line: undefined });
    expect(fileFromUri("file:///C:/x/a.md#L4")).toEqual({ path: "C:/x/a.md", line: 4 });
    expect(fileFromUri("vscode://file/Users/r/x/a.ts:12:3")).toEqual({ path: "/Users/r/x/a.ts", line: 12 });
    expect(fileFromUri("cursor://file/Users/r/x/README.md")).toEqual({ path: "/Users/r/x/README.md", line: undefined });
    expect(fileFromUri("README.md")).toEqual({ path: "README.md", line: undefined });
    expect(fileFromUri("docs/My%20Plan.md:7")).toEqual({ path: "docs/My Plan.md", line: 7 });
    expect(fileFromUri("C:\\x\\a.md")).toEqual({ path: "C:\\x\\a.md", line: undefined });
    expect(fileFromUri("https://example.com/a.md")).toBeNull();
    expect(fileFromUri("mailto:a@b.c")).toBeNull();
    expect(fileFromUri("file://server/share/a.md")).toBeNull();
  });
});

it("relativeTo shortens paths inside the root", () => {
  expect(relativeTo("/r/repo/docs/a.md", "/r/repo")).toBe("docs/a.md");
  expect(relativeTo("/r/repo2/a.md", "/r/repo")).toBe("/r/repo2/a.md");
});

it("slugs headings like GitHub", () => {
  const slug = slugger();
  expect(slug("Getting started!")).toBe("getting-started");
  expect(slug("Getting started")).toBe("getting-started-1");
  expect(slug("M0 — Shell & tiling")).toBe("m0--shell--tiling");
});
