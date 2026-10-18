import { describe, expect, it } from "vitest";
import { findComments, formatComment, insertComment, parseCommentValue, removeComment } from "./comments";

describe("review comments", () => {
  it("finds comments with their author", () => {
    const text = "# Plan\n\n<!-- @user: shorter -->\n\n- a <!-- @agent: done -->\n<!-- plain -->";
    expect(findComments(text).map((c) => [c.author, c.body, text.slice(c.start, c.end)])).toEqual([
      ["user", "shorter", "<!-- @user: shorter -->"],
      ["agent", "done", "<!-- @agent: done -->"],
    ]);
    expect(parseCommentValue(" @user: multi\nline ")).toEqual({ author: "user", body: "multi\nline" });
    expect(parseCommentValue(" plain ")).toBeNull();
  });

  it("keeps the body from closing the comment", () => {
    expect(formatComment("a --> b")).toBe("<!-- @user: a -- > b -->");
  });

  it("inserts a comment on its own between blocks", () => {
    expect(insertComment("# A\npara\n\nnext", 2, "why?")).toBe("# A\npara\n\n<!-- @user: why? -->\n\nnext");
    expect(insertComment("para\nnext", 1, "x")).toBe("para\n\n<!-- @user: x -->\n\nnext");
    expect(insertComment("para", 1, "x")).toBe("para\n\n<!-- @user: x -->");
    expect(insertComment("para\n", 1, "x")).toBe("para\n\n<!-- @user: x -->\n");
  });

  it("removes a comment and the space set around it", () => {
    const text = insertComment("# A\npara\n\nnext", 2, "why?");
    expect(removeComment(text, findComments(text)[0].start)).toBe("# A\npara\n\nnext");
    const end = insertComment("para", 1, "x");
    expect(removeComment(end, findComments(end)[0].start)).toBe("para\n");
    expect(removeComment("- a <!-- @agent: ok -->\n- b", 4)).toBe("- a\n- b");
    expect(removeComment("same", 0)).toBe("same");
  });
});
