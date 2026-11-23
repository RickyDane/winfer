import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";

export interface SelectOption {
  /** `-1` for an `<optgroup>` heading. */
  index: number;
  label: string;
  disabled: boolean;
  group?: boolean;
}

/** A page's `<select>` as `browser_select_probe` reports it (rect in page CSS pixels). */
export interface SelectProbe {
  rect: { x: number; y: number; width: number; height: number };
  options: SelectOption[];
  selected: number;
}

const MAX_HEIGHT = 320;
const MARGIN = 8;

/**
 * The option list of a page's `<select>`, drawn by the panel: headless Chrome's own popup is
 * never painted into the screencast. Sits under the select (above it when there's no room),
 * scaled with the page. Keys, wheel and paste stay here instead of reaching the page.
 */
export function SelectMenu({
  probe,
  frame,
  viewport,
  onPick,
  onClose,
}: {
  probe: SelectProbe;
  /** The shown page's size on screen, and its size in CSS pixels. */
  frame: { width: number; height: number };
  viewport: { width: number; height: number };
  onPick: (index: number) => void;
  onClose: () => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  const enabled = probe.options.filter((o) => !o.disabled);
  const [active, setActive] = useState(() => (enabled.some((o) => o.index === probe.selected) ? probe.selected : (enabled[0]?.index ?? -1)));

  const sx = frame.width / viewport.width;
  const sy = frame.height / viewport.height;
  const left = Math.max(0, Math.min(probe.rect.x * sx, frame.width - 40));
  const below = frame.height - (probe.rect.y + probe.rect.height) * sy - MARGIN;
  const above = probe.rect.y * sy - MARGIN;
  const up = below < 160 && above > below;
  const maxHeight = Math.max(80, Math.min(MAX_HEIGHT, up ? above : below));
  const position = up ? { bottom: frame.height - probe.rect.y * sy } : { top: (probe.rect.y + probe.rect.height) * sy };

  const state = useRef({ active, enabled, onPick, onClose });
  state.current = { active, enabled, onPick, onClose };

  // The list takes the keyboard so the page doesn't get the keys meant for it.
  useEffect(() => {
    const el = list.current!;
    el.focus();
    const keyDown = (e: KeyboardEvent) => {
      e.stopPropagation();
      const { active, enabled, onPick, onClose } = state.current;
      const at = enabled.findIndex((o) => o.index === active);
      const move = (to: number) => {
        const next = enabled[Math.min(Math.max(to, 0), enabled.length - 1)];
        if (next) setActive(next.index);
      };
      switch (e.key) {
        case "ArrowDown": move(at + 1); break;
        case "ArrowUp": move(at < 0 ? enabled.length - 1 : at - 1); break;
        case "Home": move(0); break;
        case "End": move(enabled.length - 1); break;
        case "Enter":
        case " ": if (active >= 0) onPick(active); else onClose(); break;
        case "Escape": onClose(); break;
        case "Tab": onClose(); return;
        default: return;
      }
      e.preventDefault();
    };
    const stop = (e: Event) => e.stopPropagation();
    el.addEventListener("keydown", keyDown);
    el.addEventListener("keyup", stop);
    el.addEventListener("paste", stop);
    el.addEventListener("wheel", stop);
    return () => {
      el.removeEventListener("keydown", keyDown);
      el.removeEventListener("keyup", stop);
      el.removeEventListener("paste", stop);
      el.removeEventListener("wheel", stop);
    };
  }, []);

  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  return (
    <>
      <div className="absolute inset-0 z-10" onMouseDown={(e) => (e.preventDefault(), onClose())} onContextMenu={(e) => (e.preventDefault(), onClose())} />
      <div
        ref={list}
        role="listbox"
        tabIndex={-1}
        style={{ left, minWidth: Math.max(probe.rect.width * sx, 120), maxWidth: Math.max(frame.width - left, 120), maxHeight, ...position }}
        className="absolute z-20 overflow-y-auto rounded-md border border-border-strong bg-panel-raised py-1 text-xs text-fg shadow-lg outline-none"
      >
        {probe.options.map((o, i) =>
          o.group ? (
            <div key={i} className="px-3 pt-1.5 pb-0.5 font-semibold text-fg-subtle">
              {o.label}
            </div>
          ) : (
            <div
              key={i}
              role="option"
              aria-selected={o.index === probe.selected}
              aria-disabled={o.disabled}
              data-index={o.index}
              onMouseEnter={() => !o.disabled && setActive(o.index)}
              onClick={() => !o.disabled && onPick(o.index)}
              className={cn(
                "flex cursor-default items-center gap-2 truncate px-3 py-1 whitespace-nowrap",
                o.disabled && "opacity-40",
                o.index === active && "bg-hover",
                o.index === probe.selected && "font-medium",
              )}
            >
              <span className="w-3 shrink-0 text-accent">{o.index === probe.selected ? "✓" : ""}</span>
              <span className="truncate">{o.label || " "}</span>
            </div>
          ),
        )}
      </div>
    </>
  );
}
