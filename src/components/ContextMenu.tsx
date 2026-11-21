import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";

export type MenuItem =
  | { label: string; run: () => void; danger?: boolean; hint?: string; disabled?: boolean }
  | "separator";

/** Right-click menu at the pointer; stays inside the window and closes on Esc, scroll or a click outside. */
export function ContextMenu({ x, y, title, items, onClose }: { x: number; y: number; title?: string; items: MenuItem[]; onClose: () => void }) {
  const root = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const r = root.current?.getBoundingClientRect();
    if (!r) return;
    setPos({
      left: Math.max(4, Math.min(x, window.innerWidth - r.width - 4)),
      top: Math.max(4, Math.min(y, window.innerHeight - r.height - 4)),
    });
  }, [x, y]);

  useEffect(() => {
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && onClose();
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    window.addEventListener("blur", onClose);
    window.addEventListener("wheel", onClose, { passive: true });
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("wheel", onClose);
    };
  }, [onClose]);

  // Drop separators at the edges and doubled ones (items hidden by conditions leave gaps).
  const shown = items.filter((item, i, all) => item !== "separator" || (i > 0 && i < all.length - 1 && all[i - 1] !== "separator"));

  return (
    <div
      ref={root}
      role="menu"
      style={pos}
      onContextMenu={(e) => e.preventDefault()}
      className="anim-menu fixed z-[70] w-60 rounded-lg border border-border-strong bg-panel-raised p-1 shadow-xl"
    >
      {title && <div className="truncate px-2 py-1 text-[11px] text-fg-subtle">{title}</div>}
      {shown.map((item, i) =>
        item === "separator" ? (
          <div key={`sep-${i}`} className="my-1 h-px bg-border" />
        ) : (
          <button
            key={item.label}
            role="menuitem"
            disabled={item.disabled}
            onClick={() => {
              onClose();
              item.run();
            }}
            className={cn(
              "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] hover:bg-hover disabled:opacity-40 disabled:hover:bg-transparent",
              item.danger ? "text-danger" : "text-fg",
            )}
          >
            <span className="flex-1 truncate">{item.label}</span>
            {item.hint && <span className="shrink-0 font-mono text-[11px] text-fg-subtle">{item.hint}</span>}
          </button>
        ),
      )}
    </div>
  );
}

/** Opens a menu at the pointer: `const [menu, open, close] = useContextMenu<T>()`. */
export function useContextMenu<T>() {
  const [menu, setMenu] = useState<{ x: number; y: number; target: T } | null>(null);
  const open = (e: React.MouseEvent, target: T) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, target });
  };
  return [menu, open, () => setMenu(null)] as const;
}
