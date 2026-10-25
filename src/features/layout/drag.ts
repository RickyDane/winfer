/** Pointer-driven drag and drop of panes (HTML5 drag events can be swallowed by the webview). */
import { create } from "zustand";
import { execute } from "@/commands/bus";
import type { DropSide } from "./tree";

export interface DropTarget {
  paneId: string;
  side: DropSide;
  rect: DOMRect;
}

interface DragState {
  paneId: string | null;
  x: number;
  y: number;
  target: DropTarget | null;
}

export const useDrag = create<DragState>(() => ({ paneId: null, x: 0, y: 0, target: null }));

const THRESHOLD = 6;
const EDGE = 0.28;

/** Which pane and zone the pointer is over: edges place the pane beside, the middle swaps. */
export function dropTargetAt(x: number, y: number, dragged: string): DropTarget | null {
  for (const el of document.querySelectorAll<HTMLElement>("[data-pane-id]")) {
    const rect = el.getBoundingClientRect();
    if (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) continue;
    const paneId = el.dataset.paneId!;
    if (paneId === dragged) return null;
    const fx = (x - rect.left) / rect.width;
    const fy = (y - rect.top) / rect.height;
    const edges: [DropSide, number][] = [
      ["left", fx],
      ["right", 1 - fx],
      ["top", fy],
      ["bottom", 1 - fy],
    ];
    const [side, distance] = edges.sort((a, b) => a[1] - b[1])[0];
    return { paneId, side: distance < EDGE ? side : "center", rect };
  }
  return null;
}

/** Call from a pane header's pointerdown; becomes a drag once the pointer moves a few pixels. */
export function beginDrag(e: React.PointerEvent, paneId: string) {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
  const startX = e.clientX;
  const startY = e.clientY;
  let active = false;

  const move = (ev: PointerEvent) => {
    if (!active && Math.hypot(ev.clientX - startX, ev.clientY - startY) < THRESHOLD) return;
    if (!active) {
      // Without this the browser keeps extending a text selection across the panes passed over.
      window.getSelection()?.removeAllRanges();
      document.body.classList.add("pane-dragging");
    }
    active = true;
    document.body.style.cursor = "grabbing";
    useDrag.setState({ paneId, x: ev.clientX, y: ev.clientY, target: dropTargetAt(ev.clientX, ev.clientY, paneId) });
  };
  const end = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", end);
    window.removeEventListener("keydown", cancel);
    document.body.style.cursor = "";
    document.body.classList.remove("pane-dragging");
    const { target } = useDrag.getState();
    useDrag.setState({ paneId: null, target: null });
    if (active && target) execute({ type: "pane.move", paneId, targetId: target.paneId, side: target.side });
  };
  const cancel = (ev: KeyboardEvent) => {
    if (ev.key !== "Escape") return;
    useDrag.setState({ target: null });
    end();
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", end);
  window.addEventListener("keydown", cancel);
}
