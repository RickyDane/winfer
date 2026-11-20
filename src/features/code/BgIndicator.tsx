import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { create, useStore } from "zustand";
import { Bot, ChevronDown, SquareTerminal } from "lucide-react";
import clsx from "clsx";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { themeById } from "@/features/theme/themes";
import { storeFor } from "./CodePane";
import { BgTasks } from "./BgTasks";
import { paneUsesWeb, useCodeWeb } from "./styles";
import type { CodeTask, createCodeStore } from "./store";

/* The pane header's activity chip: how many subagents and background shells are running
   (both winfer code tasks and winfer MCP `bg` tasks). Clicking opens their popover. */

const W = 400;
const halt = (e: React.SyntheticEvent) => e.stopPropagation();

interface McpBgPayload {
  paneId: string;
  id: string;
  title?: string;
  description?: string | null;
  cmd?: string;
  status: string;
  log?: string;
  exitCode?: number;
}

const useMcpBg = create<{ byPane: Record<string, CodeTask[]> }>(() => ({ byPane: {} }));

let mcpListenerReady = false;
function ensureMcpBgListener() {
  if (mcpListenerReady) return;
  mcpListenerReady = true;
  void listen<McpBgPayload>("bg://task", ({ payload }) => {
    if (!payload?.paneId || !payload?.id) return;
    useMcpBg.setState((s) => {
      const prev = s.byPane[payload.paneId] ?? [];
      if (payload.status !== "running") {
        const next = prev.filter((t) => t.id !== payload.id);
        return { byPane: { ...s.byPane, [payload.paneId]: next } };
      }
      const existing = prev.find((t) => t.id === payload.id);
      const task: CodeTask = {
        id: payload.id,
        title: payload.title || existing?.title || payload.id,
        description: payload.description ?? existing?.description,
        status: "running",
        last: payload.cmd || existing?.last,
        output_file: payload.log || existing?.output_file,
        startedAt: existing?.startedAt ?? Date.now(),
        recentLines: existing?.recentLines ?? [],
      };
      const next = existing ? prev.map((t) => (t.id === payload.id ? task : t)) : [...prev, task];
      return { byPane: { ...s.byPane, [payload.paneId]: next } };
    });
  }).catch(() => {});
}

export function BgIndicator({ pane }: { pane: Pane }) {
  useEffect(ensureMcpBgListener, []);
  const mcpTasks = useMcpBg((s) => s.byPane[pane.id]) ?? [];
  const isCode = pane.provider === "winfer" && paneUsesWeb(pane.id);
  if (isCode) return <CodeIndicator store={storeFor(pane)} mcpTasks={mcpTasks} paneId={pane.id} />;
  if (mcpTasks.length === 0) return null;
  return <IndicatorView tasks={mcpTasks} paneId={pane.id} />;
}

function CodeIndicator({ store, mcpTasks, paneId }: { store: ReturnType<typeof createCodeStore>; mcpTasks: CodeTask[]; paneId: string }) {
  const taskMap = useStore(store, (s) => s.tasks);
  const tasks = useMemo(() => {
    const codeList = Object.values(taskMap);
    const seen = new Set(codeList.map((t) => t.id));
    return [...codeList, ...mcpTasks.filter((t) => !seen.has(t.id))];
  }, [taskMap, mcpTasks]);
  return <IndicatorView tasks={tasks} store={store} paneId={paneId} />;
}

function IndicatorView({ tasks, store, paneId }: { tasks: CodeTask[]; store?: ReturnType<typeof createCodeStore>; paneId: string }) {
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
          "group/bg flex h-[22px] shrink-0 items-center gap-1.5 rounded-full pr-1.5 pl-[3px] text-[11px] font-medium tabular-nums transition-colors shadow-sm",
          open
            ? "bg-accent/20 text-fg"
            : "bg-accent/10 text-fg-muted hover:bg-accent/15 hover:text-fg",
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
              if (store && store.getState().tasks[id]) {
                store.getState().openTaskLog(id);
              }
            }}
            stop={(id) => {
              if (store && store.getState().tasks[id]) {
                store.getState().send({ type: "stop_task", id });
              } else {
                void invoke("bg_stop", { id });
                useMcpBg.setState((s) => ({
                  byPane: { ...s.byPane, [paneId]: (s.byPane[paneId] ?? []).filter((t) => t.id !== id) },
                }));
              }
            }}
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
