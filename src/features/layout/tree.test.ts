import { describe, expect, it } from "vitest";
import { insertDwindle, leaf, leaves, removeLeaf, split } from "./tree";

describe("insertDwindle", () => {
  it("creates a leaf in an empty layout", () => {
    expect(insertDwindle(null, "a")).toEqual(leaf("a"));
  });

  it("alternates split direction by depth", () => {
    let tree = insertDwindle(null, "a");
    tree = insertDwindle(tree, "b", "a");
    tree = insertDwindle(tree, "c", "b");
    expect(tree).toEqual(split("row", [leaf("a"), split("col", [leaf("b"), leaf("c")])]));
    expect(leaves(tree)).toEqual(["a", "b", "c"]);
  });

  it("splits the last leaf when the target is unknown", () => {
    const tree = insertDwindle(split("row", [leaf("a"), leaf("b")]), "c", "missing");
    expect(leaves(tree)).toEqual(["a", "b", "c"]);
  });
});

describe("removeLeaf", () => {
  it("collapses a split left with one child", () => {
    expect(removeLeaf(split("row", [leaf("a"), leaf("b")]), "a")).toEqual(leaf("b"));
  });

  it("renormalizes sizes of the remaining children", () => {
    const tree = removeLeaf(split("row", [leaf("a"), leaf("b"), leaf("c")], [0.5, 0.25, 0.25]), "a");
    expect(tree).toEqual(split("row", [leaf("b"), leaf("c")], [0.5, 0.5]));
  });

  it("returns null when the last pane is removed", () => {
    expect(removeLeaf(leaf("a"), "a")).toBeNull();
  });
});
