import { StickyNote, X } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/cn";
import { usePaneNotes } from "./paneNotes";

export function PaneNote({ paneId }: { paneId: string }) {
  const note = usePaneNotes((s) => s.notes[paneId] ?? "");
  const [open, setOpen] = useState(false);
  const [saved, setSaved] = useState(true);
  const [pos, setPos] = useState({ left: 0, top: 0 });
  const button = useRef<HTMLButtonElement>(null);
  const popup = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const id = useId();

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const anchor = button.current?.getBoundingClientRect();
      const rect = popup.current?.getBoundingClientRect();
      if (!anchor || !rect) return;
      setPos({
        left: Math.max(8, Math.min(anchor.right - rect.width, window.innerWidth - rect.width - 8)),
        top: Math.max(8, Math.min(anchor.bottom + 6, window.innerHeight - rect.height - 8)),
      });
    };
    place();
    input.current?.focus();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!popup.current?.contains(target) && !button.current?.contains(target)) setOpen(false);
    };
    document.addEventListener("pointerdown", outside, true);
    return () => document.removeEventListener("pointerdown", outside, true);
  }, [open]);

  const close = () => {
    setOpen(false);
    button.current?.focus();
  };

  return (
    <>
      <button
        ref={button}
        type="button"
        title={note ? "Edit pane note" : "Add pane note"}
        aria-label={note ? "Edit pane note" : "Add pane note"}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onPointerDown={(e) => e.stopPropagation()}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={() => setOpen(!open)}
        className={cn("shrink-0 rounded p-1 hover:bg-hover hover:text-fg", note ? "text-accent" : "text-fg-subtle")}
      >
        <StickyNote size={14} />
      </button>
      {open && createPortal(
        <div
          ref={popup}
          id={id}
          role="dialog"
          aria-labelledby={`${id}-title`}
          style={pos}
          onPointerDown={(e) => e.stopPropagation()}
          onMouseDown={(e) => e.stopPropagation()}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Escape") { e.preventDefault(); close(); }
          }}
          className="anim-menu fixed z-[70] flex max-h-[calc(100vh-16px)] w-80 max-w-[calc(100vw-16px)] flex-col gap-2 overflow-auto rounded-lg border border-border-strong bg-panel-raised p-3 text-fg shadow-xl"
        >
          <div className="flex items-center justify-between">
            <span id={`${id}-title`} className="text-xs font-medium">Pane note</span>
            <button type="button" aria-label="Close pane note" onClick={close} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg"><X size={14} /></button>
          </div>
          <textarea
            ref={input}
            aria-label="Pane note"
            aria-describedby={`${id}-hint`}
            placeholder="Notes to organize this task…"
            value={note}
            onChange={(e) => setSaved(usePaneNotes.getState().setNote(paneId, e.target.value))}
            rows={6}
            className="min-h-20 w-full resize-none rounded border border-border bg-panel p-2 text-xs outline-none focus:border-accent"
          />
          <p id={`${id}-hint`} className="text-[11px] text-fg-subtle">Only for you. Not shared with the agent. Saves automatically.</p>
          {!saved && <p role="alert" className="text-[11px] text-warning">Could not save locally. This note is only kept for this session.</p>}
        </div>, document.body,
      )}
    </>
  );
}
