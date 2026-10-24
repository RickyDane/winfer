import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { useStore } from "zustand";
import { Bot, ChevronDown, SquareTerminal } from "lucide-react";
import clsx from "clsx";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { themeById } from "@/features/theme/themes";
import { storeFor } from "./CodePane";
import { BgTasks } from "./BgTasks";
import { paneUsesWeb, useCodeWeb } from "./styles";
import type { createCodeStore } from "./store";

/* The pane header's activity chip for a winfer code pane: how many subagents and background
   shells are running. Clicking it opens their detailed timeline in a popover anchored to the chip. */

const W = 400;
const halt = (e: React.SyntheticEvent) => e.stopPropagation();

export function BgIndicator({ pane }: { pane: Pane }) {
  if (pane.provider !== "winfer" || !paneUsesWeb(pane.id)) return null;
  return <Indicator store={storeFor(pane)} />;
}

function Indicator({ store }: { store: ReturnType<typeof createCodeStore> }) {
  const taskMap = useStore(store, (s) => s.tasks);
  const tasks = Object.values(taskMap);
  const live = tasks.filter((t) => t.status === "running");
  const agents = live.filter((t) => t.agent).length;
  const shells = live.length - agents;
  const [open, setOpen] = useState(false);
  const btn = useRef<HTMLButtonElement>(null);

  // Nothing left to show: the popover goes with the chip.
  useEffect(() => {
    if (!live.length) setOpen(false);
  }, [live.length]);

  if (!live.length) return null;
  const parts = [agents && `${agents} subagent${agents > 1 ? "s" : ""}`, shells && `${shells} background shell${shells > 1 ? "s" : ""}`].filter(Boolean);
  return (
    <>
      <button
        ref={btn}
        onPointerDown={halt}
        onMouseDown={halt}
        onClick={() => setOpen(!open)}
        title={`${parts.join(" · ")} running\nClick for details`}
        aria-expanded={open}
        className={clsx(
          "group/bg flex h-[22px] shrink-0 items-center gap-1.5 rounded-full border pr-1.5 pl-[3px] text-[11px] font-medium tabular-nums transition-colors",
          open
            ? "border-accent/45 bg-accent/20 text-fg"
            : "border-accent/25 bg-accent/10 text-fg-muted hover:border-accent/40 hover:bg-accent/15 hover:text-fg",
        )}
      >
        {/* The kind of work going on, inside a turning ring: it reads as "busy" without blinking. */}
        <span className="relative grid size-4 place-items-center rounded-full bg-accent/15 text-accent">
          <span className="absolute inset-0 animate-spin rounded-full border-[1.5px] border-transparent border-t-accent [animation-duration:1.1s]" />
          {agents ? <Bot size={10} /> : <SquareTerminal size={10} />}
        </span>
        {agents > 0 && (
          <span className="flex items-center gap-1">
            <span className="text-fg">{agents}</span>
            <span className="hidden @[420px]:inline">{agents > 1 ? "agents" : "agent"}</span>
          </span>
        )}
        {agents > 0 && shells > 0 && <span className="size-[3px] rounded-full bg-fg-subtle/60" />}
        {shells > 0 && (
          <span className="flex items-center gap-1">
            {agents > 0 && <SquareTerminal size={10} className="text-success @[420px]:hidden" />}
            <span className="text-fg">{shells}</span>
            <span className="hidden @[420px]:inline">{shells > 1 ? "shells" : "shell"}</span>
          </span>
        )}
        <ChevronDown size={11} className={clsx("text-fg-subtle transition-transform duration-200", open && "rotate-180")} />
      </button>
      {open && btn.current && (
        <Popover anchor={btn.current} onClose={() => setOpen(false)}>
          <div className="cw-bgpop-head">
            <span>In the background</span>
            {agents > 0 && (
              <span className="cw-bgpop-count agent">
                <Bot size={11} />
                {agents}
              </span>
            )}
            {shells > 0 && (
              <span className="cw-bgpop-count shell">
                <SquareTerminal size={11} />
                {shells}
              </span>
            )}
          </div>
          <BgTasks
            tasks={tasks}
            openLog={(id) => {
              setOpen(false);
              store.getState().openTaskLog(id);
            }}
            stop={(id) => store.getState().send({ type: "stop_task", id })}
          />
        </Popover>
      )}
    </>
  );
}

/** Fixed to the viewport just below the chip, right-aligned to it and kept on screen. */
function Popover({ anchor, onClose, children }: { anchor: HTMLElement; onClose: () => void; children: React.ReactNode }) {
  const dark = useWorkspaces((s) => themeById(s.theme).mode) !== "light";
  const textScale = useCodeWeb((s) => s.textScale);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number; width: number; maxHeight: number }>();

  useLayoutEffect(() => {
    const place = () => {
      const r = anchor.getBoundingClientRect();
      const width = Math.min(W, window.innerWidth - 16);
      const left = Math.max(8, Math.min(r.right - width, window.innerWidth - width - 8));
      const top = r.bottom + 6;
      setPos({ top, left, width, maxHeight: Math.max(160, window.innerHeight - top - 12) });
    };
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [anchor]);

  useEffect(() => {
    const down = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!ref.current?.contains(t) && !anchor.contains(t)) onClose();
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("mousedown", down, true);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("mousedown", down, true);
      document.removeEventListener("keydown", key);
    };
  }, [anchor, onClose]);

  return createPortal(
    <div
      ref={ref}
      onPointerDown={halt}
      className={clsx("cw-pane cw-bgpop", !dark && "light")}
      style={{ ...pos, colorScheme: dark ? "dark" : "light", "--cw-text": textScale, visibility: pos ? undefined : "hidden" } as CSSProperties}
    >
      {children}
    </div>,
    document.body,
  );
}
