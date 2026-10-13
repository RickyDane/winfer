import { useRef, type ReactNode } from "react";
import { cn } from "@/lib/cn";
import type { LayoutNode, Path } from "./tree";

const GUTTER = 8;
const MIN_FRACTION = 0.08;

interface Props {
  node: LayoutNode;
  renderLeaf: (paneId: string) => ReactNode;
  onResize: (path: Path, sizes: number[]) => void;
  path?: Path;
}

/**
 * Renders the layout tree as nested flex splits with draggable gutters.
 * Terminals inside leaves are re-parented, not recreated, so re-rendering here is cheap.
 */
export function TileView({ node, renderLeaf, onResize, path = [] }: Props) {
  const container = useRef<HTMLDivElement>(null);
  if (node.kind === "leaf") return <>{renderLeaf(node.paneId)}</>;

  const horizontal = node.dir === "row";

  const startDrag = (i: number) => (e: React.PointerEvent) => {
    e.preventDefault();
    const el = container.current!;
    const rect = el.getBoundingClientRect();
    const total = (horizontal ? rect.width : rect.height) - GUTTER * (node.children.length - 1);
    const start = horizontal ? e.clientX : e.clientY;
    const initial = [...node.sizes];
    const pair = initial[i] + initial[i + 1];

    const move = (ev: PointerEvent) => {
      const delta = ((horizontal ? ev.clientX : ev.clientY) - start) / total;
      const a = Math.min(Math.max(initial[i] + delta, MIN_FRACTION), pair - MIN_FRACTION);
      const sizes = [...initial];
      sizes[i] = a;
      sizes[i + 1] = pair - a;
      onResize(path, sizes);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.style.cursor = "";
    };
    document.body.style.cursor = horizontal ? "col-resize" : "row-resize";
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div ref={container} className="flex h-full min-h-0 w-full min-w-0" style={{ flexDirection: horizontal ? "row" : "column" }}>
      {node.children.map((child, i) => (
        <div key={i} className="contents">
          {i > 0 && (
            <div
              onPointerDown={startDrag(i - 1)}
              className={cn(
                "group relative shrink-0",
                horizontal ? "cursor-col-resize" : "cursor-row-resize",
              )}
              style={horizontal ? { width: GUTTER } : { height: GUTTER }}
            >
              <div
                className={cn(
                  "absolute rounded-full bg-accent opacity-0 transition-opacity group-hover:opacity-60",
                  horizontal ? "inset-y-2 left-[3px] w-0.5" : "inset-x-2 top-[3px] h-0.5",
                )}
              />
            </div>
          )}
          <div className="flex min-h-0 min-w-0" style={{ flex: `${node.sizes[i]} 1 0` }}>
            <TileView node={child} renderLeaf={renderLeaf} onResize={onResize} path={[...path, i]} />
          </div>
        </div>
      ))}
    </div>
  );
}
