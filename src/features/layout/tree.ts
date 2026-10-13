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

export type Path = number[];

const MIN_SIZE = 0.1;

/** Replaces the sizes of the split at `path` (used by gutter drags). */
export function setSizesAt(root: LayoutNode, path: Path, sizes: number[]): LayoutNode {
  if (root.kind === "leaf") return root;
  if (path.length === 0) return { ...root, sizes };
  const [i, ...rest] = path;
  return { ...root, children: root.children.map((c, j) => (j === i ? setSizesAt(c, rest, sizes) : c)) };
}

/** Index path from the root to a leaf, or null. */
export function pathTo(root: LayoutNode, paneId: string): Path | null {
  if (root.kind === "leaf") return root.paneId === paneId ? [] : null;
  for (let i = 0; i < root.children.length; i++) {
    const sub = pathTo(root.children[i], paneId);
    if (sub) return [i, ...sub];
  }
  return null;
}

function nodeAt(root: LayoutNode, path: Path): LayoutNode {
  return path.reduce((node, i) => (node.kind === "split" ? node.children[i] : node), root);
}

/**
 * Grows (delta > 0) or shrinks the pane along an axis by moving space from/to its neighbor in the
 * deepest enclosing split of that direction. "row" = width, "col" = height.
 */
export function resizePane(root: LayoutNode, paneId: string, axis: SplitDir, delta: number): LayoutNode {
  const path = pathTo(root, paneId);
  if (!path) return root;
  for (let depth = path.length - 1; depth >= 0; depth--) {
    const splitPath = path.slice(0, depth);
    const node = nodeAt(root, splitPath);
    if (node.kind !== "split" || node.dir !== axis) continue;
    const i = path[depth];
    const j = i + 1 < node.children.length ? i + 1 : i - 1;
    const sizes = [...node.sizes];
    const d = delta > 0 ? Math.min(delta, sizes[j] - MIN_SIZE) : Math.max(delta, MIN_SIZE - sizes[i]);
    if (d === 0 || Number.isNaN(d)) return root;
    sizes[i] += d;
    sizes[j] -= d;
    return setSizesAt(root, splitPath, sizes);
  }
  return root;
}

export function swapLeaves(root: LayoutNode, a: string, b: string): LayoutNode {
  if (root.kind === "leaf") {
    if (root.paneId === a) return leaf(b);
    if (root.paneId === b) return leaf(a);
    return root;
  }
  return { ...root, children: root.children.map((c) => swapLeaves(c, a, b)) };
}

export function equalize(root: LayoutNode): LayoutNode {
  if (root.kind === "leaf") return root;
  const children = root.children.map(equalize);
  return { ...root, children, sizes: children.map(() => 1 / children.length) };
}

export const PRESETS = ["auto", "main-stack", "grid", "columns", "rows"] as const;
export type Preset = (typeof PRESETS)[number];

/** Rebuilds the layout for the given panes (in order) from a preset. */
export function buildPreset(preset: Preset, ids: string[]): LayoutNode | null {
  if (ids.length === 0) return null;
  if (ids.length === 1) return leaf(ids[0]);
  switch (preset) {
    case "columns":
      return split("row", ids.map(leaf));
    case "rows":
      return split("col", ids.map(leaf));
    case "main-stack":
      return split("col", [leaf(ids[0]), ids.length === 2 ? leaf(ids[1]) : split("row", ids.slice(1).map(leaf))], [0.6, 0.4]);
    case "grid": {
      const cols = Math.ceil(Math.sqrt(ids.length));
      const rows: LayoutNode[] = [];
      for (let i = 0; i < ids.length; i += cols) {
        const row = ids.slice(i, i + cols);
        rows.push(row.length === 1 ? leaf(row[0]) : split("row", row.map(leaf)));
      }
      return rows.length === 1 ? rows[0] : split("col", rows);
    }
    case "auto":
      return ids.reduce<LayoutNode | null>((tree, id, i) => insertDwindle(tree, id, i > 0 ? ids[i - 1] : null), null);
  }
}
