import type { ReactNode } from "react";
import type { LayoutNode } from "./tree";

const GAP = 8;

/**
 * Renders a layout tree as nested flex splits. Static for M0; M2 adds draggable gutters,
 * keyboard resize and re-parenting of terminal DOM nodes so panes never remount.
 */
export function TileView({ node, renderLeaf }: { node: LayoutNode; renderLeaf: (paneId: string) => ReactNode }) {
  if (node.kind === "leaf") return <>{renderLeaf(node.paneId)}</>;

  return (
    <div className="flex h-full w-full min-h-0 min-w-0" style={{ flexDirection: node.dir === "row" ? "row" : "column", gap: GAP }}>
      {node.children.map((child, i) => (
        <div key={i} className="flex min-h-0 min-w-0" style={{ flex: `${node.sizes[i]} 1 0` }}>
          <TileView node={child} renderLeaf={renderLeaf} />
        </div>
      ))}
    </div>
  );
}
