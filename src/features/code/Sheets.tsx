/** Panels that open over the transcript: what the TUI shows in its pickers and info notices. */
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Activity,
  Bot,
  CircleHelp,
  Gauge,
  History,
  Info,
  KeyRound,
  MessageCircleQuestion,
  Plug,
  Square,
  Sparkles,
  Shrink,
  X,
  Zap,
  Check,
  Star,
  ChevronLeft,
  ChevronRight,
  SquareTerminal,
  ShieldCheck,
} from "lucide-react";
import clsx from "clsx";
import { useCode, type Sheet } from "./store";
import { COMMANDS, MODES, type TaskEv } from "./protocol";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { EFFORT_HINTS, EFFORT_LABELS, EffortIcon, effortOptions } from "./EffortIcon";
import { useCodeWeb } from "./styles";
import { BACKENDS, baseSpec, fastSpec, isFast, modelName, modelTags } from "./names";

/** Token counts: 950 · 12.3k · 184k · 2.76M. */
export function fmt(n: number) {
  const trim = (x: string) => x.replace(/\.0+$/, "");
  if (n >= 1_000_000) return `${trim((n / 1_000_000).toFixed(n >= 10_000_000 ? 1 : 2))}M`;
  if (n >= 1000) return `${trim((n / 1000).toFixed(n >= 100_000 ? 0 : 1))}k`;
  return String(n);
}

function ago(secs: number) {
  const d = Date.now() / 1000 - secs;
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)}m ago`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
  return `${Math.floor(d / 86400)}d ago`;
}

function Frame({ title, icon, children, wide }: { title: string; icon: ReactNode; children: ReactNode; wide?: boolean }) {
  const close = useCode((s) => s.setSheet);
  return (
    <div className="cw-sheet-wrap" onMouseDown={(e) => e.target === e.currentTarget && close(null)}>
      <div className={clsx("cw-sheet", wide && "wide")} role="dialog" aria-label={title}>
        <header>
          {icon}
          <b>{title}</b>
          <span className="grow" />
          <button className="cw-icon-btn sm" onClick={() => close(null)} title="Close (esc)">
            <X size={14} />
          </button>
        </header>
        <div className="cw-sheet-body">{children}</div>
      </div>
    </div>
  );
}

function Stat({ label, value, hint }: { label: string; value: ReactNode; hint?: string }) {
  return (
    <div className="cw-stat-cell" title={hint}>
      <small>{label}</small>
      <b>{value}</b>
    </div>
  );
}

/** `/cost`: the context window with its compaction thresholds, and the session's token totals. */
export function useRate() {
  const rate = useCode((s) => s.rate);
  const busy = useCode((s) => s.busy);
  if (!busy || !rate || rate.at - rate.startAt < 800) return null;
  return ((rate.tokens - rate.start) / (rate.at - rate.startAt)) * 1000;
}

function UsageSheet() {
  const { usage, trimAt, compactAt, model, send, setSheet } = useCode();
  const rate = useRate();
  const pct = usage.window ? (usage.context / usage.window) * 100 : 0;
  const hit = usage.input ? (usage.cached / usage.input) * 100 : 0;
  const fresh = Math.max(0, usage.input - usage.cached);
  const total = usage.input + usage.output || 1;
  const chatgpt = model.startsWith("chatgpt");
  return (
    <Frame title="Usage & context" icon={<Gauge size={15} />} wide>
      <section className="cw-usage-block">
        <div className="cw-ctx-head">
          <span>Context window</span>
          <b>
            {fmt(usage.context)} <small>/ {fmt(usage.window)}</small>
          </b>
          <span className="grow" />
          <span className={clsx("cw-ctx-pct", pct > 80 && "hot")}>{pct.toFixed(0)}%</span>
        </div>
        <div className="cw-ctx-bar">
          <i style={{ width: `${Math.min(pct, 100)}%` }} />
          {trimAt > 0 && <em style={{ left: `${trimAt}%` }} data-label={`micro-compact ${trimAt}%`} />}
          {compactAt > 0 && <em className="full" style={{ left: `${compactAt}%` }} data-label={`compact ${compactAt}%`} />}
        </div>
        <div className="cw-ctx-legend">
          <span>
            <i className="used" /> in context
          </span>
          <span>{fmt(Math.max(0, usage.window - usage.context))} free</span>
          <span className="grow" />
          <button className="cw-link" onClick={() => setSheet("compaction")}>
            Compaction: {trimAt ? `trim ${trimAt}%` : "trim off"} · {compactAt ? `compact ${compactAt}%` : "compact off"}
          </button>
        </div>
      </section>

      <section className="cw-usage-block">
        <div className="cw-ctx-head">
          <span>Session tokens</span>
          <b>{fmt(usage.input + usage.output)}</b>
        </div>
        <div className="cw-split-bar">
          <i className="cached" style={{ width: `${(usage.cached / total) * 100}%` }} title={`${fmt(usage.cached)} cached input`} />
          <i className="fresh" style={{ width: `${(fresh / total) * 100}%` }} title={`${fmt(fresh)} uncached input`} />
          <i className="out" style={{ width: `${(usage.output / total) * 100}%` }} title={`${fmt(usage.output)} output`} />
        </div>
        <div className="cw-ctx-legend">
          <span>
            <i className="cached" /> cached {fmt(usage.cached)}
          </span>
          <span>
            <i className="fresh" /> input {fmt(fresh)}
          </span>
          <span>
            <i className="out" /> output {fmt(usage.output)}
          </span>
        </div>
      </section>

      <div className="cw-stat-grid">
        <Stat label="Cache hits" value={usage.input ? `${hit.toFixed(0)}%` : "—"} hint="Share of input read from the prompt cache" />
        <Stat label="This turn" value={usage.turn ? `${fmt(usage.turn)} out` : "—"} />
        <Stat label="Speed" value={rate ? `${rate.toFixed(0)} tok/s` : "—"} />
        <Stat
          label="Cost"
          value={usage.cost != null ? `$${usage.cost.toFixed(4)}` : chatgpt ? "ChatGPT plan" : "—"}
          hint={usage.cost != null ? "API pricing; your plan may cover it" : undefined}
        />
      </div>
      <div className="cw-sheet-actions">
        <button className="cw-btn" onClick={() => send({ type: "compact" })}>
          <Shrink size={13} /> Compact now
        </button>
      </div>
    </Frame>
  );
}

const PERCENTS = [0, 50, 60, 70, 75, 80, 85, 90, 95];

function CompactionSheet() {
  const { trimAt, compactAt, send } = useCode();
  const row = (label: string, hint: string, value: number, key: "trim" | "compact") => (
    <div className="cw-field">
      <div>
        <b>{label}</b>
        <small>{hint}</small>
      </div>
      <div className="cw-seg">
        {PERCENTS.map((p) => (
          <button key={p} className={clsx(p === value && "sel")} onClick={() => send({ type: "set_compaction", [key]: p })}>
            {p ? `${p}%` : "off"}
          </button>
        ))}
      </div>
    </div>
  );
  return (
    <Frame title="Compaction" icon={<Shrink size={15} />} wide>
      {row("Micro-compact", "Trims old tool output once context reaches this share. Claude Code and Codex restart briefly between turns to apply it.", trimAt, "trim")}
      {row("Full compact", "Summarizes the conversation once context reaches this share.", compactAt, "compact")}
    </Frame>
  );
}

function StatusSheet() {
  const s = useCode();
  const rows: [string, ReactNode][] = [
    ["Session", <code key="s">{s.session ?? "…"}</code>],
    ["Backend session", <code key="b">{s.backendSession ?? "not started"}</code>],
    ["Model", `${modelName(s.model, useCodeWeb.getState().resolvedModels, s.actualModel)} (${s.actualModel ?? s.model})`],
    ["Mode", MODES.find((m) => m.id === s.mode)?.label ?? s.mode],
    ["Effort", s.effort],
    ["Directory", <code key="d">{s.cwd ?? "—"}</code>],
    ["Branch", s.branch ?? "—"],
  ];
  return (
    <Frame title="Status" icon={<Info size={15} />}>
      <dl className="cw-dl">
        {rows.map(([k, v]) => (
          <div key={k}>
            <dt>{k}</dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>
    </Frame>
  );
}

function TaskRow({ t }: { t: TaskEv }) {
  const send = useCode((s) => s.send);
  const open = useCode((s) => s.openTaskLog);
  const status = t.status ?? "running";
  return (
    <div className={clsx("cw-task-row", status)}>
      <span className="dot" />
      <button className="cw-task-open" onClick={() => open(t.id)} title="Show its output">
        <b>{t.title ?? t.id}</b>
        <small>
          {t.agent ? `${t.agent} · ${t.tools ?? 0} tools` : "background shell"} · {status}
          {t.exit_code != null && status !== "running" ? ` · exit ${t.exit_code}` : ""}
          {t.last ? ` · ${t.last}` : ""}
        </small>
        {t.description && <p>{t.description}</p>}
      </button>
      {status === "running" && (
        <button className="cw-btn small" onClick={() => send({ type: "stop_task", id: t.id })}>
          <Square size={10} fill="currentColor" /> Stop
        </button>
      )}
      <ChevronRight size={14} className="cw-task-chev" />
    </div>
  );
}

/** `/agents` and `/tasks`: running first, then finished, newest first; click one for its output. */
function AgentsSheet() {
  const all = Object.values(useCode((s) => s.tasks)).reverse();
  const order = (list: TaskEv[]) => [...list.filter((t) => (t.status ?? "running") === "running"), ...list.filter((t) => (t.status ?? "running") !== "running")];
  const shells = order(all.filter((t) => !t.agent));
  const agents = order(all.filter((t) => t.agent));
  return (
    <Frame title="Agents & background tasks" icon={<Bot size={15} />} wide>
      {all.length === 0 && <p className="cw-empty">No subagents or background shells in this session yet.</p>}
      {shells.length > 0 && (
        <section>
          <div className="cw-sheet-label">Background shells · ctrl+b</div>
          {shells.map((t) => (
            <TaskRow key={t.id} t={t} />
          ))}
        </section>
      )}
      {agents.length > 0 && (
        <section>
          <div className="cw-sheet-label">Subagents · ctrl+g</div>
          {agents.map((t) => (
            <TaskRow key={t.id} t={t} />
          ))}
        </section>
      )}
    </Frame>
  );
}

/** One background task's output, like the TUI's task view: live tail while it runs. */
function TaskLogSheet() {
  const id = useCode((s) => s.logTask);
  const task = useCode((s) => (id ? s.tasks[id] : undefined));
  const log = useCode((s) => s.taskLog);
  const items = useCode((s) => s.items);
  const send = useCode((s) => s.send);
  const setSheet = useCode((s) => s.setSheet);
  const running = (task?.status ?? "running") === "running";
  const pre = useRef<HTMLPreElement>(null);
  const follow = useRef(true);
  useEffect(() => {
    if (!id) return;
    send({ type: "task_log", id });
    if (!running) return;
    const t = setInterval(() => send({ type: "task_log", id }), 400);
    return () => clearInterval(t);
  }, [id, running, send]);
  // Without a log file (subagents), the output its card has.
  const card = task?.tool_id ? items.find((i) => i.kind === "tool" && i.key === task.tool_id) : undefined;
  const text = log?.text ?? (card && card.kind === "tool" ? card.output : null) ?? "";
  useEffect(() => {
    if (follow.current && pre.current) pre.current.scrollTop = pre.current.scrollHeight;
  }, [text]);
  if (!id || !task) return null;
  const status = task.status ?? "running";
  return (
    <Frame title={task.title ?? id} icon={task.agent ? <Bot size={15} /> : <SquareTerminal size={15} />} wide>
      <div className="cw-log-head">
        <span className={clsx("cw-log-status", status)}>
          <span className="dot" />
          {status}
          {task.exit_code != null && status !== "running" ? ` · exit ${task.exit_code}` : ""}
        </span>
        <span className="dim">{task.agent ? `${task.agent} · ${task.tools ?? 0} tools` : "background shell"}</span>
        <span className="grow" />
        <button className="cw-btn small" onClick={() => setSheet("agents")}>
          <ChevronLeft size={12} /> All tasks
        </button>
        {status === "running" && (
          <button className="cw-btn small" onClick={() => send({ type: "stop_task", id })}>
            <Square size={10} fill="currentColor" /> Stop
          </button>
        )}
      </div>
      {task.description && <p className="cw-log-desc">{task.description}</p>}
      <pre
        ref={pre}
        className="cw-log"
        onScroll={(e) => {
          const el = e.currentTarget;
          follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
      >
        {text || (task.agent ? (task.last ? `Working… last step: ${task.last}` : "No output yet.") : running ? "Waiting for output…" : "No output.")}
        {running && <span className="cw-log-caret" />}
      </pre>
      {log?.file && <small className="cw-log-file">{log.file}</small>}
    </Frame>
  );
}

function SessionsSheet() {
  const sessions = useCode((s) => s.sessions);
  const current = useCode((s) => s.session);
  const send = useCode((s) => s.send);
  const close = useCode((s) => s.setSheet);
  return (
    <Frame title="Resume a session" icon={<History size={15} />} wide>
      {sessions === null && <p className="cw-empty">Loading…</p>}
      {sessions?.length === 0 && <p className="cw-empty">No saved sessions in this folder yet.</p>}
      <div className="cw-list">
        {sessions?.map((x) => (
          <button
            key={x.id}
            className={clsx("cw-list-item", x.id === current && "sel")}
            onClick={() => {
              send({ type: "resume", id: x.id });
              close(null);
            }}
          >
            <b>{x.title || "(untitled)"}</b>
            <small>
              {ago(x.updated)} · {x.messages} message{x.messages === 1 ? "" : "s"} · {x.model}
            </small>
          </button>
        ))}
      </div>
    </Frame>
  );
}

let btwSeq = 0;
function BtwSheet() {
  const btw = useCode((s) => s.btw);
  const send = useCode((s) => s.send);
  const [q, setQ] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  return (
    <Frame title="Side question" icon={<MessageCircleQuestion size={15} />} wide>
      <p className="cw-empty">Answered from the transcript by a separate call — the running turn isn't interrupted.</p>
      <div className="cw-list">
        {[...btw].reverse().map((b) => (
          <div key={b.id} className="cw-btw">
            <b>{b.question}</b>
            {b.answer ? <p>{b.answer}</p> : b.error ? <p className="err">{b.error}</p> : <span className="cw-shimmer">Thinking…</span>}
          </div>
        ))}
      </div>
      <form
        className="cw-inline-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (!q.trim()) return;
          send({ type: "btw", id: `btw-ui-${++btwSeq}`, question: q.trim() });
          setQ("");
        }}
      >
        <input ref={input} value={q} onChange={(e) => setQ(e.target.value)} placeholder="Ask about this session…" />
        <button className="cw-btn primary small" type="submit">
          Ask
        </button>
      </form>
    </Frame>
  );
}


/** `/model`: a tab per backend (with its logo), its models below. Opens from the status line. */
const logoFor = (spec: string) => BACKENDS.find((b) => spec.startsWith(b.id + "/"))?.logo ?? "codex";

export function ModelPicker() {
  const { models, model, actualModel, send, setSheet } = useCode();
  const resolved = useCodeWeb((s) => s.resolvedModels);
  const favorites = useCodeWeb((s) => s.favoriteModels);
  const setPrefs = useCodeWeb((s) => s.set);
  const toggleFavorite = (spec: string) =>
    setPrefs({
      favoriteModels: favorites.some((f) => baseSpec(f) === spec) ? favorites.filter((f) => baseSpec(f) !== spec) : [...favorites, spec],
    });
  const current = model.split("/")[0];
  // Fast twins aren't rows: the status line's Fast toggle switches between a model and its twin.
  const picked = baseSpec(model);
  const pick = (spec: string) => (isFast(model) && fastSpec(spec, models)) || spec;
  const rows = models.filter((m) => !isFast(m.spec) || !models.some((b) => b.spec === baseSpec(m.spec)));
  const backends = BACKENDS.filter((b) => rows.some((m) => m.spec.startsWith(b.id + "/")));
  const favModels = [...new Set(favorites.map(baseSpec))]
    .map((f) => rows.find((m) => m.spec === f))
    .filter((m): m is (typeof models)[number] => !!m);
  // Opens on Favorites when the current model is one of them, else on its backend.
  const [tab, setTab] = useState(() => (favModels.some((m) => m.spec === picked) ? "favorites" : current));
  const active = tab === "favorites" || backends.some((b) => b.id === tab) ? tab : (backends[0]?.id ?? current);
  const tabs = useRef<HTMLDivElement>(null);
  const [glider, setGlider] = useState<{ top: number; height: number } | null>(null);
  useEffect(() => {
    const el = tabs.current?.querySelector<HTMLElement>(`[data-tab="${active}"]`);
    if (el) setGlider({ top: el.offsetTop, height: el.offsetHeight });
  }, [active, backends.length]);
  const [query, setQuery] = useState("");
  const all = active === "favorites" ? favModels : rows.filter((m) => m.spec.startsWith(active + "/"));
  const q = query.trim().toLowerCase();
  const list = q ? all.filter((m) => `${m.label} ${m.spec} ${m.detail}`.toLowerCase().includes(q)) : all;
  return (
    <div className="cw-model-picker">
      {backends.length === 0 ? (
        <p className="cw-empty">Loading models…</p>
      ) : (
        <>
          <div className="cw-tabs" ref={tabs} role="tablist">
            {glider && <span className="cw-tab-glider" style={glider} />}
            <button
              data-tab="favorites"
              role="tab"
              aria-selected={active === "favorites"}
              title="Favorites"
              aria-label="Favorites"
              className={clsx("cw-tab fav", active === "favorites" && "sel")}
              onClick={() => setTab("favorites")}
            >
              <Star size={15} fill={favorites.length ? "currentColor" : "none"} />
            </button>
            <span className="cw-tabs-sep" />
            {backends.map((b) => (
              <button key={b.id} data-tab={b.id} role="tab" aria-selected={b.id === active} title={b.label} aria-label={b.label} className={clsx("cw-tab", b.id === active && "sel")} onClick={() => setTab(b.id)}>
                <ProviderIcon provider={b.logo} size={16} />
              </button>
            ))}
          </div>
          <div className="cw-list cw-model-list" key={active}>
            {all.length > 10 && (
              <input className="cw-model-search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder={`Search ${all.length} models…`} autoFocus />
            )}
            {q && list.length === 0 && <p className="cw-empty cw-fav-empty">No models match.</p>}
            {active === "favorites" && list.length === 0 && (
              <p className="cw-empty cw-fav-empty">
                No favorites yet. Star a model on its provider's tab to keep it here.
              </p>
            )}
            {list.map((m) => {
              const fav = favModels.includes(m);
              return (
                <div key={m.spec} className={clsx("cw-list-item model", m.spec === picked && "sel")}>
                  <button
                    className="cw-model-pick"
                    onClick={() => {
                      send({ type: "set_model", model: pick(m.spec) });
                      setSheet(null);
                    }}
                  >
                    {active === "favorites" && <ProviderIcon provider={logoFor(m.spec)} size={13} />}
                    <span>
                      <b>
                        {modelName(m.spec, resolved, m.spec === picked ? actualModel : null)}
                        {modelTags(m.spec).map((t) => (
                          <em key={t} className="cw-model-tag">
                            {t}
                          </em>
                        ))}
                      </b>
                      {m.detail && <small>{m.detail}</small>}
                    </span>
                  </button>
                  {m.spec === picked && <Check size={14} className="cw-check" />}
                  <button
                    className={clsx("cw-star", fav && "on")}
                    title={fav ? "Remove from favorites" : "Add to favorites"}
                    aria-pressed={fav}
                    onClick={() => toggleFavorite(m.spec)}
                  >
                    <Star size={13} fill={fav ? "currentColor" : "none"} />
                  </button>
                </div>
              );
            })}
          </div>
        </>
      )}
    </div>
  );
}

function ListSheet({ kind }: { kind: "model" | "effort" | "mode" }) {
  const { models, model, effort, mode, send, setSheet } = useCode();
  const items =
    kind === "model"
      ? models.map((m) => ({ id: m.spec, label: m.label, hint: m.detail }))
      : kind === "effort"
        ? effortOptions(models, model).map((e) => ({ id: e, label: EFFORT_LABELS[e] ?? e, hint: EFFORT_HINTS[e] }))
        : MODES.map((m) => ({ id: m.id, label: m.label, hint: m.hint }));
  const value = kind === "model" ? model : kind === "effort" ? effort : mode;
  const icon = kind === "model" ? <Sparkles size={15} /> : kind === "effort" ? <Zap size={15} /> : <ShieldCheck size={15} />;
  return (
    <Frame title={kind === "model" ? "Model" : kind === "effort" ? "Reasoning effort" : "Permission mode"} icon={icon}>
      {items.length === 0 && <p className="cw-empty">Loading models…</p>}
      <div className="cw-list">
        {items.map((i) => (
          <button
            key={i.id}
            className={clsx("cw-list-item", i.id === value && "sel")}
            onClick={() => {
              if (kind === "model") send({ type: "set_model", model: i.id });
              else if (kind === "effort") send({ type: "set_effort", effort: i.id === "default" ? null : i.id });
              else send({ type: "set_mode", mode: i.id as (typeof MODES)[number]["id"] });
              setSheet(null);
            }}
          >
            {kind === "effort" ? (
              <span className="cw-pop-row">
                <EffortIcon effort={i.id} size={16} />
                <span>
                  <b>{i.label}</b>
                  {i.hint && <small>{i.hint}</small>}
                </span>
              </span>
            ) : (
              <>
                <b>{i.label}</b>
                {i.hint && <small>{i.hint}</small>}
              </>
            )}
          </button>
        ))}
      </div>
    </Frame>
  );
}

const KEYS: [string, string][] = [
  ["enter", "send (steers the running turn)"],
  ["shift+enter", "new line"],
  ["esc", "interrupt / close a panel"],
  ["shift+tab", "cycle permission mode"],
  ["y · a · n", "allow · always allow · deny an approval (empty composer)"],
  ["/", "commands"],
];

function HelpSheet({ onCommand }: { onCommand: (c: string) => void }) {
  return (
    <Frame title="Keys & commands" icon={<CircleHelp size={15} />} wide>
      <dl className="cw-dl keys">
        {KEYS.map(([k, d]) => (
          <div key={k}>
            <dt>
              <kbd>{k}</kbd>
            </dt>
            <dd>{d}</dd>
          </div>
        ))}
      </dl>
      <div className="cw-list cmds">
        {COMMANDS.map(([c, d]) => (
          <button key={c} className="cw-list-item row" onClick={() => onCommand(c)}>
            <code>{c}</code>
            <small>{d}</small>
          </button>
        ))}
      </div>
    </Frame>
  );
}

function McpSheet() {
  const mcp = useCode((s) => s.mcp);
  return (
    <Frame title="MCP servers" icon={<Plug size={15} />}>
      {mcp.length === 0 ? (
        <p className="cw-empty">No MCP servers. Add them under "mcpServers" in winfer code's config.json (inside winfer, its own server is added automatically).</p>
      ) : (
        <div className="cw-list">
          {mcp.map((m) => (
            <div key={m} className="cw-list-item static">
              <b>
                <Activity size={12} /> {m}
              </b>
            </div>
          ))}
        </div>
      )}
    </Frame>
  );
}

function LoginSheet() {
  return (
    <Frame title="Sign in" icon={<KeyRound size={15} />}>
      <dl className="cw-dl">
        <div>
          <dt>ChatGPT</dt>
          <dd>winfer → Settings → AI → Sign in with ChatGPT</dd>
        </div>
        <div>
          <dt>Claude Code</dt>
          <dd>
            run <code>claude</code> once and <code>/login</code>
          </dd>
        </div>
        <div>
          <dt>Codex</dt>
          <dd>
            run <code>codex login</code>
          </dd>
        </div>
      </dl>
    </Frame>
  );
}

export function Sheets({ onCommand }: { onCommand: (c: string) => void }) {
  const sheet = useCode((s) => s.sheet);
  const setSheet = useCode((s) => s.setSheet);
  useEffect(() => {
    if (!sheet) return;
    const on = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setSheet(null);
      }
    };
    window.addEventListener("keydown", on, true);
    return () => window.removeEventListener("keydown", on, true);
  }, [sheet, setSheet]);
  const views: Record<Sheet, ReactNode> = {
    usage: <UsageSheet />,
    compaction: <CompactionSheet />,
    status: <StatusSheet />,
    agents: <AgentsSheet />,
    tasklog: <TaskLogSheet />,
    sessions: <SessionsSheet />,
    btw: <BtwSheet />,
    help: <HelpSheet onCommand={onCommand} />,
    mcp: <McpSheet />,
    login: <LoginSheet />,
    // Opens as a popover from the status line (see CodePane's Footer).
    model: null,
    effort: <ListSheet kind="effort" />,
    mode: <ListSheet kind="mode" />,
  };
  return sheet ? <>{views[sheet]}</> : null;
}

