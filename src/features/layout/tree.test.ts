import { describe, expect, it } from "vitest";
import { buildPreset, equalize, insertDwindle, leaf, leaves, movePane, pathTo, removeLeaf, resizePane, split, swapLeaves } from "./tree";

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

describe("resizePane", () => {
  const tree = split("row", [leaf("a"), split("col", [leaf("b"), leaf("c")])]);

  it("grows width by taking from the right neighbor", () => {
    const next = resizePane(tree, "a", "row", 0.1);
    expect(next.kind === "split" && next.sizes[0]).toBeCloseTo(0.6);
  });

  it("uses the left neighbor for the last child", () => {
    const next = resizePane(tree, "b", "row", 0.1);
    expect(next.kind === "split" && next.sizes).toEqual([expect.closeTo(0.4), expect.closeTo(0.6)]);
  });

  it("resizes height in the nearest col split", () => {
    const next = resizePane(tree, "c", "col", 0.2);
    expect(pathTo(next, "c")).toEqual([1, 1]);
    const inner = next.kind === "split" ? next.children[1] : next;
    expect(inner.kind === "split" && inner.sizes).toEqual([expect.closeTo(0.3), expect.closeTo(0.7)]);
  });

  it("clamps to a minimum size", () => {
    const next = resizePane(tree, "a", "row", 5);
    expect(next.kind === "split" && next.sizes[1]).toBeCloseTo(0.1);
  });

  it("is a no-op without a split on that axis", () => {
    expect(resizePane(split("row", [leaf("a"), leaf("b")]), "a", "col", 0.1)).toEqual(split("row", [leaf("a"), leaf("b")]));
  });
});

describe("other ops", () => {
  it("swaps leaves", () => {
    expect(leaves(swapLeaves(split("row", [leaf("a"), leaf("b")]), "a", "b"))).toEqual(["b", "a"]);
  });

  it("equalizes nested sizes", () => {
    expect(equalize(split("row", [leaf("a"), leaf("b")], [0.8, 0.2]))).toEqual(split("row", [leaf("a"), leaf("b")]));
  });

  it("builds presets that keep pane order", () => {
    const ids = ["a", "b", "c", "d", "e"];
    for (const preset of ["auto", "main-stack", "grid", "columns", "rows"] as const) {
      expect(leaves(buildPreset(preset, ids)!)).toEqual(ids);
    }
    expect(buildPreset("main-stack", ["a", "b", "c"])).toEqual(
      split("col", [leaf("a"), split("row", [leaf("b"), leaf("c")])], [0.6, 0.4]),
    );
  });
});

describe("movePane (drag and drop)", () => {
  const tree = split("row", [leaf("a"), split("col", [leaf("b"), leaf("c")])]);

  it("drops on an edge to split the target", () => {
    expect(movePane(tree, "c", "a", "bottom")).toEqual(split("row", [split("col", [leaf("a"), leaf("c")]), leaf("b")]));
  });

  it("joins a split that already runs that way instead of nesting", () => {
    expect(movePane(tree, "c", "a", "left")).toEqual(split("row", [leaf("c"), leaf("a"), leaf("b")], [0.25, 0.25, 0.5]));
  });

  it("swaps on the center", () => {
    expect(leaves(movePane(tree, "a", "c", "center"))).toEqual(["c", "b", "a"]);
  });

  it("ignores dropping a pane on itself", () => {
    expect(movePane(tree, "a", "a", "right")).toBe(tree);
  });
});
