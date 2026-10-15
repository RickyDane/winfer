import { useState } from "react";

/** A drag in progress: `id` would land before the item now at `index` (== count: at the end). */
export interface SortDrag {
  id: string;
  index: number;
}

/**
 * Drag to reorder with pointer events (Tauri's native file drop swallows HTML5 drag and drop).
 * Items are the siblings marked `data-sort-id`; call `begin` from an item's onPointerDown.
 * A press that doesn't move a few pixels stays a plain click.
 */
export function useSortable(onMove: (id: string, index: number) => void) {
  const [drag, setDrag] = useState<SortDrag | null>(null);

  const begin = (e: React.PointerEvent, id: string) => {
    if (e.button !== 0) return;
    const list = (e.currentTarget as HTMLElement).closest<HTMLElement>("[data-sort-id]")?.parentElement;
    if (!list) return;
    const startY = e.clientY;
    let current: SortDrag | null = null;
    const items = () => Array.from(list.children).filter((el): el is HTMLElement => el instanceof HTMLElement && el.dataset.sortId != null);

    const move = (ev: PointerEvent) => {
      if (!current && Math.abs(ev.clientY - startY) < 5) return;
      const index = items().filter((el) => {
        const r = el.getBoundingClientRect();
        return r.top + r.height / 2 < ev.clientY;
      }).length;
      if (current?.index === index) return;
      current = { id, index };
      setDrag(current);
      document.body.style.cursor = "grabbing";
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (!current) return;
      document.body.style.cursor = "";
      setDrag(null);
      const from = items().findIndex((el) => el.dataset.sortId === id);
      const to = current.index > from ? current.index - 1 : current.index;
      if (from >= 0 && to !== from) onMove(id, to);
      // The release would also click the item it ends on.
      const swallow = (c: MouseEvent) => c.stopPropagation();
      window.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  /** Marks where the dragged item would land: a line above item `i`, or below the last one. */
  const dropLine = (i: number, count: number) => {
    if (!drag) return null;
    if (drag.index === i) return "top";
    if (i === count - 1 && drag.index === count) return "bottom";
    return null;
  };

  return { drag, begin, dropLine };
}
