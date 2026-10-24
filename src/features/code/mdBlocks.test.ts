import { createElement, Fragment } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { describe, expect, it } from "vitest";
import { splitBlocks } from "./mdBlocks";

const html = (blocks: string[]) =>
  renderToStaticMarkup(
    createElement(Fragment, null, ...blocks.map((b, i) => createElement(ReactMarkdown, { key: i, remarkPlugins: [remarkGfm] }, b))),
  ).replace(/>\s+</g, "><");

const SAMPLES = [
  "# Title\n\nA paragraph with `code` and a [link](https://example.com).\n\n- one\n- two\n\nDone.",
  "Steps:\n\n1. First\n\n2. Second, loose\n\n   Continued in the item.\n\n3. Third\n\nAfter the list.",
  "Code:\n\n```ts\nconst a = 1;\n\nconst b = 2;\n```\n\nAnd after.",
  "- item\n\n  ```bash\n  npm install\n\n  npm test\n  ```\n\n- next item\n\nend",
  "| a | b |\n| - | - |\n| 1 | 2 |\n\n> quote\n> more\n\n> another quote\n\n---\n\nTitle\n=====\n\ntext",
  "Use ```inline``` here\n\nthen a paragraph\n\n~~~\nraw\n\n~~~\n\nlast",
  "    indented code\n\n    still code\n\nparagraph",
  "Unclosed fence:\n\n```js\nlet x = 1;\n\nlet y = 2;",
];

describe("splitBlocks", () => {
  it("keeps every character", () => {
    for (const s of SAMPLES) expect(splitBlocks(s).join("\n")).toBe(s);
  });

  it("renders the same as the whole text", () => {
    for (const s of SAMPLES) expect(html(splitBlocks(s))).toBe(html([s]));
  });

  it("renders the same at every point while streaming in", () => {
    for (const s of SAMPLES)
      for (let n = 1; n <= s.length; n++) {
        const prefix = s.slice(0, n);
        expect(html(splitBlocks(prefix)), JSON.stringify(prefix)).toBe(html([prefix]));
      }
  });

  it("cuts between top-level blocks", () => {
    expect(splitBlocks("a\n\nb\n\n# c")).toEqual(["a\n", "b\n", "# c"]);
  });

  it("doesn't cut inside fences, lists or indented continuations", () => {
    expect(splitBlocks("```\na\n\nb\n```")).toHaveLength(1);
    expect(splitBlocks("1. a\n\n2. b")).toHaveLength(1);
    expect(splitBlocks("- a\n\n  more of a")).toHaveLength(1);
    expect(splitBlocks("Text\n\n```js\nopen\n\nstill open")).toHaveLength(2);
  });

  it("leaves text with definitions whole", () => {
    expect(splitBlocks("See [x].\n\n[x]: https://example.com")).toHaveLength(1);
    expect(splitBlocks("Note[^1].\n\n[^1]: The note.")).toHaveLength(1);
  });
});
