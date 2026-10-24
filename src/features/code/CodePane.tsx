import { useEffect, useRef, useState, type CSSProperties } from "react";
import {
  Bot,
  ChevronDown,
  CircleCheck,
  CircleDashed,
  GitBranch,
  ListTodo,
  LoaderCircle,
  Zap,
} from "lucide-react";
import clsx from "clsx";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { themeById } from "@/features/theme/themes";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { nameFromHistory, setTask } from "@/features/progress/progress";
import { CodeStoreContext, createCodeStore, useCode } from "./store";
import { Transcript, Collapse, useFresh } from "./Transcript";
import { LinkPaneContext } from "./Links";
import { Composer } from "./Composer";
import { MockTransport } from "./mock";
import { CODE_WEB_DEMO, stepTextScale, useCodeWeb } from "./styles";
import { ModelPicker, Sheets, SubagentPicker, SUBAGENT_BACKENDS } from "./Sheets";
import { backendOf, baseSpec, fastSpec, isFast, modelName } from "./names";
import { Lightbox } from "./Lightbox";
import { EFFORT_HINTS, EFFORT_LABELS, EffortIcon, effortOptions } from "./EffortIcon";
import { runCommand } from "./commands";
import { PtyTransport } from "./transport";
import { MODES } from "./protocol";
import { ModeIcon } from "./ModeIcon";
import { loadPlanSidebar, usePlanSidebar } from "./planSidebar";
import "./code.css";

/**
 * The agent's todos (running subagents and shells sit in the pane header's indicator). `strip` docks above the composer like the TUI's plan strip;
 * `side` is the right-docked sidebar (Plan sidebar setting), which CSS only shows when the pane is wide
 * enough — the strip takes over otherwise.
 */
function Plan({ side }: { side?: boolean }) {
  const todos = useCode((s) => s.todos);
  const [open, setOpen] = useState(true);
  const asking = useCode((s) => s.approvals.length > 0);
  const show = useCodeWeb((s) => s.showPlan);
  const sidebar = usePlanSidebar((s) => s.on);
  useEffect(loadPlanSidebar, []);
  if (!show || !todos.length || (side && !sidebar)) return null;
  const done = todos.filter((t) => t.status === "completed").length;
  const current = todos.find((t) => t.status === "in_progress");
  const expanded = side || (open && !asking);
  return (
    <div className={clsx(side ? "cw-side" : "cw-plan", "cw-enter")}>
      <button
        className="cw-plan-head"
        onClick={side ? undefined : () => setOpen(!open)}
        disabled={side}
      >
        <ListTodo size={14} />
        <span className="cw-plan-title">
          {side
            ? done === todos.length
              ? "Plan complete"
              : "Plan"
            : current
              ? current.content
              : done === todos.length
                ? "Plan complete"
                : "Plan"}
        </span>
        <span className="cw-plan-bar">
          <i style={{ width: `${(done / todos.length) * 100}%` }} />
        </span>
        <span className="cw-plan-count">
          {done}/{todos.length}
        </span>
        {!side && (
          <ChevronDown size={13} className={clsx("cw-chev-d", expanded && "open")} />
        )}
      </button>
      <Collapse open={expanded}>
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
    </div>
  );
}

function Menu({
  label,
  value,
  items,
  onPick,
  className,
  title,
  children,
}: {
  label: string;
  className?: string;
  title?: string;
  value: string;
  items: { id: string; label: string; hint?: string; icon?: React.ReactNode }[];
  onPick: (id: string) => void;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    // A long list that has to scroll opens on the current choice.
    ref.current
      ?.querySelector(".cw-pop-item.sel")
      ?.scrollIntoView({ block: "nearest" });
    const off = (e: MouseEvent) =>
      !ref.current?.contains(e.target as Node) && setOpen(false);
    window.addEventListener("mousedown", off);
    return () => window.removeEventListener("mousedown", off);
  }, [open]);
  return (
    <div className="cw-menu" ref={ref}>
      <button
        className={clsx("cw-foot-item", className, open && "active")}
        onClick={() => setOpen(!open)}
        title={title ?? label}
      >
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
    const off = (e: MouseEvent) =>
      !ref.current?.contains(e.target as Node) && setSheet(null);
    window.addEventListener("mousedown", off);
    return () => window.removeEventListener("mousedown", off);
  }, [open, setSheet]);
  return (
    <div className="cw-menu" ref={ref}>
      <button
        className={clsx("cw-foot-item", open && "active")}
        onClick={() => setSheet(open ? null : "model")}
        title="Model (/model)"
      >
        <ProviderIcon provider={backendOf(model)?.logo ?? "winfer"} size={12} />
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

/** What subagents run on, anchored to the status line (also what `/subagent-model` opens). */
function SubagentMenu({ model }: { model: string }) {
  const open = useCode((s) => s.sheet === "subagent-model");
  const { subModel, subEffort, setSheet } = useCode();
  const resolved = useCodeWeb((s) => s.resolvedModels);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const off = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setSheet(null);
    window.addEventListener("mousedown", off);
    return () => window.removeEventListener("mousedown", off);
  }, [open, setSheet]);
  const backend = model.split("/")[0];
  if (!SUBAGENT_BACKENDS.includes(backend)) return null;
  const label = subModel ? modelName(`${backend}/${subModel}`, resolved) : null;
  return (
    <div className="cw-menu" ref={ref}>
      <button
        className={clsx("cw-foot-item", open && "active", !label && "dim")}
        onClick={() => setSheet(open ? null : "subagent-model")}
        title={label ? `Subagents run on ${label}${subEffort ? ` · ${subEffort}` : ""} (/subagent-model)` : "Subagents run on the main model (/subagent-model)"}
      >
        <Bot size={12} />
        {label ?? "same"}
      </button>
      {open && (
        <div className="cw-pop cw-model-pop">
          <SubagentPicker />
        </div>
      )}
    </div>
  );
}

/** The TUI's bottom status line: mode, model, effort, context, branch. */
function Footer({ pane }: { pane: Pane }) {
  const { mode, model, models, actualModel, effort, usage, send, setSheet } = useCode();
  const fast = isFast(model);
  const fastTwin = fastSpec(model, models);
  const prefs = useCodeWeb();
  const fresh = useFresh();
  const pct = usage.window ? usage.context / usage.window : 0;
  const m = MODES.find((x) => x.id === mode) ?? MODES[0];
  const modelLabel = model
    ? modelName(model, prefs.resolvedModels, actualModel)
    : "…";
  return (
    <div className="cw-foot">
      <Menu
        label="Permission mode"
        title={`${m.hint} · shift+tab to cycle`}
        className={clsx("mode", `mode-${mode}`)}
        value={mode}
        items={MODES.map((x) => ({
          id: x.id,
          label: x.label,
          hint: x.hint,
          icon: <ModeIcon mode={x.id} size={16} />,
        }))}
        onPick={(id) =>
          send({ type: "set_mode", mode: id as (typeof MODES)[number]["id"] })
        }
      >
        <ModeIcon mode={mode} size={12} />
        {m.label}
      </Menu>
      <ModelMenu label={modelLabel} model={model} />
      {fastTwin && (
        <button
          className={clsx("cw-foot-item cw-fast", fast && "on")}
          aria-pressed={fast}
          title={fast ? "Fast mode on: 2x speed, increased usage. Click to turn off" : "Fast mode: 2x speed, increased usage"}
          onClick={() => send({ type: "set_model", model: fast ? baseSpec(model) : fastTwin })}
        >
          <Zap size={12} fill={fast ? "currentColor" : "none"} />
          Fast
        </button>
      )}
      <Menu
        label="Reasoning effort"
        value={effort}
        items={effortOptions(models, model).map((e) => ({
          id: e,
          label: EFFORT_LABELS[e] ?? e,
          hint: EFFORT_HINTS[e],
          icon: <EffortIcon effort={e} size={16} />,
        }))}
        onPick={(e) =>
          send({ type: "set_effort", effort: e === "default" ? null : e })
        }
      >
        <EffortIcon effort={effort} size={12} />
        <span className="dim">{EFFORT_LABELS[effort] ?? effort}</span>
      </Menu>
      <SubagentMenu model={model} />
      <span className="grow" />
      {pane.worktree && (
        <span className="cw-foot-item dim">
          <GitBranch size={12} />
          {pane.worktree.branch}
        </span>
      )}
      {!fresh && ((prefs.showCost && usage.cost != null) || prefs.showContext) && (
        <button
          className={clsx("cw-foot-item", prefs.showContext && pct > 0.8 && "hot")}
          onClick={() => setSheet("usage")}
          title={[
            prefs.showCost && usage.cost != null
              ? "Session cost (API pricing; your plan may cover it)"
              : null,
            prefs.showContext
              ? `Context ${usage.context.toLocaleString()} / ${usage.window.toLocaleString()} tokens`
              : null,
            "click for the breakdown",
          ]
            .filter(Boolean)
            .join(" · ")}
        >
          {prefs.showCost && usage.cost != null && (
            <span className="dim">${usage.cost.toFixed(2)}</span>
          )}
          {prefs.showContext && (
            <>
              <span className="cw-meter">
                <i style={{ width: `${pct * 100}%` }} />
              </span>
              {Math.round(pct * 100)}%
            </>
          )}
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
    useWorkspaces
      .getState()
      .setPaneStatus(
        paneId,
        asking ? "needs-input" : busy ? "working" : "idle",
      );
  }, [paneId, busy, asking]);
  return null;
}

const stores = new Map<string, ReturnType<typeof createCodeStore>>();

/** Ends a pane's session view (the PTY itself is killed with the pane, see closeTerminal). */
export function disposeCodePane(paneId: string) {
  stores.delete(paneId);
}

/** One store per pane that outlives remounts (moving tiles, switching workspaces). */
export function storeFor(pane: Pane) {
  let s = stores.get(pane.id);
  if (!s) {
    s = createCodeStore();
    stores.set(pane.id, s);
    if (CODE_WEB_DEMO) {
      // The browser preview: a scripted session.
      s.getState().attach(new MockTransport());
      const prompt =
        pane.prompt ??
        "Add exponential backoff to retry() and make it cancellable";
      setTimeout(() => s!.getState().submit(prompt), 3500);
    } else {
      s.getState().attach(new PtyTransport(pane));
    }
    trackTitle(pane.id, s);
  }
  return s;
}

/**
 * Names the pane like a terminal agent's: each message the user sends (typed, the start prompt,
 * or sent from elsewhere) becomes its task, and the first one gets an LLM title for the header.
 * A resumed session without a saved title is named from its first restored message.
 */
function trackTitle(paneId: string, store: ReturnType<typeof createCodeStore>) {
  store.subscribe((st, prev) => {
    if (st.items === prev.items) return;
    const appended = st.items.length > prev.items.length && st.items[prev.items.length - 1] === prev.items.at(-1);
    // Undo and /clear shrink the list: nothing new was said.
    const added = appended ? st.items.slice(prev.items.length) : st.items;
    const users = added.filter((i) => i.kind === "user");
    const live = appended && users.filter((i) => i.at > 0).at(-1);
    if (live) return setTask(paneId, live.text);
    const restored = users.find((i) => i.at === 0 && i.text.trim());
    if (restored) nameFromHistory(paneId, restored.text);
  });
}

/** winfer code as a web UI: what a winfer pane shows instead of the terminal TUI. */
export function CodePane({ pane, focused }: { pane: Pane; focused: boolean }) {
  const store = storeFor(pane);
  const dark = useWorkspaces((s) => themeById(s.theme).mode) !== "light";
  const glass = useWorkspaces((s) => s.glassPanes);
  const compact = useCodeWeb((s) => s.compact);
  const textScale = useCodeWeb((s) => s.textScale);
  const root = useRef<HTMLDivElement>(null);

  // Ctrl/⌘ + wheel (and trackpad pinch, which arrives as Ctrl + wheel) resizes the text.
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    let pinch = 0;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      e.stopPropagation();
      pinch -= e.deltaY;
      if (Math.abs(pinch) < 40) return;
      stepTextScale(Math.sign(pinch));
      pinch = 0;
    };
    el.addEventListener("wheel", onWheel, { passive: false, capture: true });
    return () => el.removeEventListener("wheel", onWheel, { capture: true });
  }, []);

  useEffect(() => {
    if (focused) root.current?.querySelector("textarea")?.focus();
  }, [focused]);

  return (
    <CodeStoreContext.Provider value={store}>
      <LinkPaneContext.Provider value={pane.id}>
        <div
          ref={root}
          className={clsx(
            "cw-pane",
            !dark && "light",
            glass && "glass",
            compact && "compact",
          )}
          style={{ colorScheme: dark ? "dark" : "light", "--cw-text": textScale } as CSSProperties}
        >
          {CODE_WEB_DEMO && <StatusSync paneId={pane.id} />}
          <Transcript dark={dark} />
          <div className="cw-dock">
            <Plan />
            <Composer focused={focused} />
            <Footer pane={pane} />
          </div>
          <Plan side />
          <Lightbox />
          <Sheets
            onCommand={(c) => {
              store.getState().setSheet(null);
              runCommand(c, store);
            }}
          />
        </div>
      </LinkPaneContext.Provider>
    </CodeStoreContext.Provider>
  );
}
