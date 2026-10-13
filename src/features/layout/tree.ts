/**
 * Layout tree for the tiled pane area. Pure data + functions so it can be unit-tested
 * and persisted. M2 adds resize, swap and presets.
 */
export type SplitDir = "row" | "col";

export type LayoutNode =
  | { kind: "leaf"; paneId: string }
  | { kind: "split"; dir: SplitDir; sizes: number[]; children: LayoutNode[] };

export const leaf = (paneId: string): LayoutNode => ({ kind: "leaf", paneId });

export const split = (dir: SplitDir, children: LayoutNode[], sizes?: number[]): LayoutNode => ({
  kind: "split",
  dir,
  children,
  sizes: sizes ?? children.map(() => 1 / children.length),
});

export function leaves(node: LayoutNode): string[] {
  return node.kind === "leaf" ? [node.paneId] : node.children.flatMap(leaves);
}

/**
 * Dwindle insertion: the target leaf is split in two, alternating direction by depth
 * (row at even depths, col at odd), so repeated inserts spiral like i3/Hyprland dwindle.
 * Without a target the new pane splits the last leaf.
 */
export function insertDwindle(root: LayoutNode | null, newId: string, targetId?: string | null): LayoutNode {
  if (!root) return leaf(newId);
  const target = targetId && leaves(root).includes(targetId) ? targetId : leaves(root).at(-1)!;

  const visit = (node: LayoutNode, depth: number): LayoutNode => {
    if (node.kind === "leaf") {
      return node.paneId === target ? split(depth % 2 === 0 ? "row" : "col", [node, leaf(newId)]) : node;
    }
    return { ...node, children: node.children.map((c) => visit(c, depth + 1)) };
  };
  return visit(root, 0);
}

/** Removes a leaf; splits left with one child collapse into that child. */
export function removeLeaf(root: LayoutNode | null, paneId: string): LayoutNode | null {
  if (!root) return null;
  if (root.kind === "leaf") return root.paneId === paneId ? null : root;

  const kept: { node: LayoutNode; size: number }[] = [];
  root.children.forEach((child, i) => {
    const next = removeLeaf(child, paneId);
    if (next) kept.push({ node: next, size: root.sizes[i] });
  });
  if (kept.length === 0) return null;
  if (kept.length === 1) return kept[0].node;
  const total = kept.reduce((sum, k) => sum + k.size, 0);
  return { ...root, children: kept.map((k) => k.node), sizes: kept.map((k) => k.size / total) };
}
