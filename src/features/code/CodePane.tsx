import { useEffect, useRef, useState } from "react";
import { Bot, SquareTerminal, ChevronDown, CircleCheck, CircleDashed, GitBranch, ListTodo, LoaderCircle, Square } from "lucide-react";
import clsx from "clsx";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { themeById } from "@/features/theme/themes";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { CodeStoreContext, createCodeStore, useCode } from "./store";
import { Transcript, Collapse } from "./Transcript";
import { Composer } from "./Composer";
import { MockTransport } from "./mock";
import { CODE_WEB_DEMO, useCodeWeb } from "./styles";
import { ModelPicker, Sheets } from "./Sheets";
import { EFFORT_HINTS, EFFORT_LABELS, EffortIcon } from "./EffortIcon";
import { runCommand } from "./commands";
import { PtyTransport } from "./transport";
import { MODES } from "./protocol";
import "./code.css";

/** The agent's todos and running subagents, docked above the composer like the TUI's plan strip. */
function PlanStrip() {
  const todos = useCode((s) => s.todos);
  const taskMap = useCode((s) => s.tasks);
  const send = useCode((s) => s.send);
  const [open, setOpen] = useState(true);
  const asking = useCode((s) => s.approvals.length > 0);
  const openLog = useCode((s) => s.openTaskLog);
  const running = Object.values(taskMap).filter((t) => t.status === "running");
  const show = useCodeWeb((s) => s.showPlan);
  if (!show || (!todos.length && !running.length)) return null;
  const done = todos.filter((t) => t.status === "completed").length;
  const current = todos.find((t) => t.status === "in_progress");
  return (
    <div className="cw-plan cw-enter">
      {todos.length > 0 && (
        <>
          <button className="cw-plan-head" onClick={() => setOpen(!open)}>
            <ListTodo size={14} />
            <span className="cw-plan-title">{current ? current.content : done === todos.length ? "Plan complete" : "Plan"}</span>
            <span className="cw-plan-bar">
              <i style={{ width: `${(done / todos.length) * 100}%` }} />
            </span>
            <span className="cw-plan-count">
              {done}/{todos.length}
            </span>
            <ChevronDown size={13} className={clsx("cw-chev-d", open && !asking && "open")} />
          </button>
          <Collapse open={open && !asking}>
            <ul className="cw-todos">
              {todos.map((t) => (
                <li key={t.content} className={t.status}>
                  {t.status === "completed" ? (
                    <CircleCheck size={14} />
                  ) : t.status === "in_progress" ? (
                    <LoaderCircle size={14} className="spin" />
                  ) : (
                    <CircleDashed size={14} />
                  )}
                  <span>{t.content}</span>
                </li>
              ))}
            </ul>
          </Collapse>
        </>
      )}
      {running.map((t) => (
        <div key={t.id} className="cw-task running clickable" onClick={() => openLog(t.id)} title="Show its output">
          {t.agent ? <Bot size={14} /> : <SquareTerminal size={14} />}
          <b>{t.title}</b>
          <small>
            {t.agent ? `${t.agent} · ${t.tools ?? 0} tools` : "background shell"}
            {t.last ? ` · ${t.last}` : ""}
          </small>
          <button
            className="cw-icon-btn sm"
            title="Stop"
            onClick={(e) => {
              e.stopPropagation();
              send({ type: "stop_task", id: t.id });
            }}
          >
            <Square size={10} fill="currentColor" />
          </button>
        </div>
      ))}
    </div>
  );
}

function Menu({ label, value, items, onPick, children }: { label: string; value: string; items: { id: string; label: string; hint?: string; icon?: React.ReactNode }[]; onPick: (id: string) => void; children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    // A long list that has to scroll opens on the current choice.
    ref.current?.querySelector(".cw-pop-item.sel")?.scrollIntoView({ block: "nearest" });
    const off = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    window.addEventListener("mousedown", off);
    return () => window.removeEventListener("mousedown", off);
  }, [open]);
  return (
    <div className="cw-menu" ref={ref}>
      <button className={clsx("cw-foot-item", open && "active")} onClick={() => setOpen(!open)} title={label}>
        {children}
      </button>
      {open && (
        <div className="cw-pop">
          <div className="cw-pop-label">{label}</div>
          <div className="cw-pop-list">
            {items.map((i) => (
              <button
                key={i.id}
                className={clsx("cw-pop-item", i.id === value && "sel")}
                onClick={() => {
                  onPick(i.id);
                  setOpen(false);
                }}
              >
                {i.icon ? (
                  <span className="cw-pop-row">
                    {i.icon}
                    <span>
                      {i.label}
                      {i.hint && <small>{i.hint}</small>}
                    </span>
                  </span>
                ) : (
                  <>
                    <span>{i.label}</span>
                    {i.hint && <small>{i.hint}</small>}
                  </>
                )}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

/** The model picker, anchored to the status line (also what `/model` opens). */
function ModelMenu({ label, model }: { label: string; model: string }) {
  const open = useCode((s) => s.sheet === "model");
  const setSheet = useCode((s) => s.setSheet);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const off = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setSheet(null);
    window.addEventListener("mousedown", off);
    return () => window.removeEventListener("mousedown", off);
  }, [open, setSheet]);
  return (
    <div className="cw-menu" ref={ref}>
      <button className={clsx("cw-foot-item", open && "active")} onClick={() => setSheet(open ? null : "model")} title="Model (/model)">
        <ProviderIcon provider={model.startsWith("claude") ? "claude" : model.startsWith("chatgpt") ? "openai" : "codex"} size={12} />
        {label}
      </button>
      {open && (
        <div className="cw-pop cw-model-pop">
          <ModelPicker />
        </div>
      )}
    </div>
  );
}

/** Highest first, so the lowest level sits at the bottom, next to the status line. */
const EFFORTS = ["max", "xhigh", "high", "medium", "low", "minimal", "default"];

/** The TUI's bottom status line: mode, model, effort, context, branch. */
function Footer({ pane }: { pane: Pane }) {
  const { mode, model, models, effort, usage, send, setSheet } = useCode();
  const prefs = useCodeWeb();
  const pct = usage.window ? usage.context / usage.window : 0;
  const m = MODES.find((x) => x.id === mode) ?? MODES[0];
  const cycle = () => send({ type: "set_mode", mode: MODES[(MODES.indexOf(m) + 1) % MODES.length].id });
  const modelLabel = models.find((x) => x.spec === model)?.label ?? (model || "…");
  return (
    <div className="cw-foot">
      <button className={clsx("cw-foot-item mode", `mode-${mode}`)} onClick={cycle} title={`${m.hint} · shift+tab`}>
        {m.label}
      </button>
      <ModelMenu label={modelLabel} model={model} />
      <Menu
        label="Reasoning effort"
        value={effort}
        items={EFFORTS.map((e) => ({ id: e, label: EFFORT_LABELS[e] ?? e, hint: EFFORT_HINTS[e], icon: <EffortIcon effort={e} size={16} /> }))}
        onPick={(e) => send({ type: "set_effort", effort: e === "default" ? null : e })}
      >
        <EffortIcon effort={effort} size={12} />
        <span className="dim">{EFFORT_LABELS[effort] ?? effort}</span>
      </Menu>
      <span className="grow" />
      {pane.worktree && (
        <span className="cw-foot-item dim">
          <GitBranch size={12} />
          {pane.worktree.branch}
        </span>
      )}
      {prefs.showCost && usage.cost != null && (
        <button className="cw-foot-item dim" onClick={() => setSheet("usage")} title="Session cost (API pricing; your plan may cover it)">
          ${usage.cost.toFixed(2)}
        </button>
      )}
      {prefs.showContext && (
        <button
          className={clsx("cw-foot-item", pct > 0.8 && "hot")}
          onClick={() => setSheet("usage")}
          title={`Context ${usage.context.toLocaleString()} / ${usage.window.toLocaleString()} tokens · click for the breakdown`}
        >
          <span className="cw-meter">
            <i style={{ width: `${pct * 100}%` }} />
          </span>
          {Math.round(pct * 100)}%
        </button>
      )}
    </div>
  );
}

/** Keeps the pane's status dot in step with the session (working while a turn runs). */
function StatusSync({ paneId }: { paneId: string }) {
  const busy = useCode((s) => s.busy);
  const asking = useCode((s) => s.approvals.length > 0);
  useEffect(() => {
    useWorkspaces.getState().setPaneStatus(paneId, asking ? "needs-input" : busy ? "working" : "idle");
  }, [paneId, busy, asking]);
  return null;
}

const stores = new Map<string, ReturnType<typeof createCodeStore>>();

/** Ends a pane's session view (the PTY itself is killed with the pane, see closeTerminal). */
export function disposeCodePane(paneId: string) {
  stores.delete(paneId);
}

/** One store per pane that outlives remounts (moving tiles, switching workspaces). */
function storeFor(pane: Pane) {
  let s = stores.get(pane.id);
  if (!s) {
    s = createCodeStore();
    stores.set(pane.id, s);
    if (CODE_WEB_DEMO) {
      // The browser preview: a scripted session.
      s.getState().attach(new MockTransport());
      const prompt = pane.prompt ?? "Add exponential backoff to retry() and make it cancellable";
      setTimeout(() => s!.getState().submit(prompt), 3500);
    } else {
      s.getState().attach(new PtyTransport(pane));
    }
  }
  return s;
}

/** winfer code as a web UI: what a winfer pane shows instead of the terminal TUI. */
export function CodePane({ pane, focused }: { pane: Pane; focused: boolean }) {
  const store = storeFor(pane);
  const dark = useWorkspaces((s) => themeById(s.theme).mode) !== "light";
  const glass = useWorkspaces((s) => s.glassPanes);
  const compact = useCodeWeb((s) => s.compact);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (focused) root.current?.querySelector("textarea")?.focus();
  }, [focused]);

  return (
    <CodeStoreContext.Provider value={store}>
      <div ref={root} className={clsx("cw-pane", !dark && "light", glass && "glass", compact && "compact")} style={{ colorScheme: dark ? "dark" : "light" }}>
        {CODE_WEB_DEMO && <StatusSync paneId={pane.id} />}
        <Transcript dark={dark} />
        <div className="cw-dock">
          <PlanStrip />
          <Composer focused={focused} />
          <Footer pane={pane} />
        </div>
        <Sheets
          onCommand={(c) => {
            store.getState().setSheet(null);
            runCommand(c, store);
          }}
        />
      </div>
    </CodeStoreContext.Provider>
  );
}

