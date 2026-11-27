import { createPortal } from "react-dom";
import { useCallback, useMemo, useRef, useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { ago, go, haptic, useClient, useTick, vtName } from "../hooks";
import { isFreeText, sortPanes, type AgentStatus, type Pane, type RequestOption } from "../protocol";
import { Avatar, ConnNotice, Icon, STATUS_LABEL, linkState, providerInfo } from "./common";
import { activeIsServer } from "../pairing";
import { forMachine } from "../multi";
import { matchesPane } from "./paneSearch";
import { Segmented, SwipeCard, useFlip, usePullToRefresh } from "./widgets";

type Filter = "all" | "needs" | "working" | "done";
const MATCH: Record<Filter, (p: Pane) => boolean> = {
  all: () => true,
  needs: (p) => p.status === "needs-input" || p.status === "error",
  working: (p) => p.status === "working",
  done: (p) => p.status === "done" || p.status === "idle",
};
const GROUPS: { title: string; statuses: AgentStatus[] }[] = [
  { title: "Needs you", statuses: ["needs-input", "error"] },
  { title: "Working", statuses: ["working"] },
  { title: "Finished", statuses: ["done"] },
  { title: "Idle", statuses: ["idle"] },
];

function countStatuses(panes: Pane[]): Record<AgentStatus, number> {
  const c: Record<AgentStatus, number> = { "needs-input": 0, working: 0, done: 0, error: 0, idle: 0 };
  for (const p of panes) c[p.status]++;
  return c;
}

/** One muted line under the title: what needs you first, then the rest. */
function summary(counts: Record<AgentStatus, number>, total: number): string {
  if (!total) return "No agents yet";
  const parts = [
    counts["needs-input"] && `${counts["needs-input"]} ${counts["needs-input"] === 1 ? "needs" : "need"} you`,
    counts.error && `${counts.error} ${counts.error === 1 ? "error" : "errors"}`,
    counts.working && `${counts.working} working`,
  ].filter(Boolean);
  return [...parts, `${total} ${total === 1 ? "agent" : "agents"}`].join(" · ");
}

function preview(p: Pane): string | null {
  if (p.request?.kind === "approval") return p.request.text.split("\n")[0];
  if (p.request) return p.request.text;
  const text = p.status === "done" ? (p.reply ?? p.message) : p.message;
  return text ? text.replace(/[#*`_>]/g, "").trim() : null;
}

export function Inbox({ client }: { client: DesktopClient }) {
  const state = useClient(client);
  useTick(30_000);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [menu, setMenu] = useState<Pane | null>(null);
  const [confirm, setConfirm] = useState(false);
  const openMenu = (p: Pane) => {
    setConfirm(false);
    setMenu(p);
  };
  const closeSession = (p: Pane) => {
    // A working agent is stopped mid-turn: ask once more.
    if (p.status === "working" && !confirm) return setConfirm(true);
    haptic([10, 30, 10]);
    setMenu(null);
    void client.request({ t: "pane.close", paneId: p.id }).catch(() => {});
  };
  const scroller = useRef<HTMLDivElement>(null);
  const ptr = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);

  const found = sortPanes(state.panes).filter((p) => matchesPane(p, query, providerInfo(p.provider)?.name, state.threads[p.id]?.messages));
  const counts = useMemo(() => countStatuses(state.panes), [state.panes]);
  // The filters count what the search left; the headline keeps counting everything.
  const foundCounts = countStatuses(found);
  const link = linkState(state);
  const multi = state.machines.length > 1;
  const desktop = multi ? "All machines" : (state.welcome?.desktop ?? client.name);
  // With several machines the bar says how many answer; one that doesn't gets its own notice.
  const away = state.machines.filter((m) => linkState(forMachine(state, m.id)) !== "online");
  // A failed machine says why (it won't heal by itself); one that's away or offline comes back on its own.
  const failed = away.filter((m) => m.error);
  const gone = away.filter((m) => !m.error && ["away", "offline"].includes(linkState(forMachine(state, m.id))));
  const shown = found.filter(MATCH[filter]);
  useFlip(list, shown.map((p) => `${p.id}:${p.status}`).join() + filter);

  const refresh = useCallback(() => client.request({ t: "panes.get" }).catch(() => client.reconnect()), [client]);
  usePullToRefresh(scroller, ptr, refresh);

  const act = (p: Pane, t: "approve" | "decline" | "interrupt") => {
    haptic(t === "approve" ? [10, 30, 10] : 8);
    void client.request({ t, paneId: p.id }).catch(() => {});
  };

  const choose = (p: Pane, o: RequestOption) => {
    haptic([8, 30, 8]);
    void client.request({ t: "choose", paneId: p.id, option: o.n }).catch(() => {});
  };

  const open = (p: Pane) => go(`/pane/${encodeURIComponent(p.id)}`);
  const canStart = state.workspaces.length > 0 && state.providers.length > 0;
  let i = 0;
  const card = (p: Pane) => {
    const pv = preview(p);
    const approval = p.request?.kind === "approval";
    const question = p.request?.kind === "question" ? p.request : null;
    const picks = question?.options?.filter((o) => !isFreeText(o)) ?? [];
    const info = providerInfo(p.provider);
    return (
      <SwipeCard
        key={p.id}
        index={i++}
        status={p.status}
        className={p.request ? "request" : undefined}
        onTap={() => open(p)}
        onLongPress={() => openMenu(p)}
        right={approval ? { label: "Approve", icon: "check", run: () => act(p, "approve") } : undefined}
        left={approval ? { label: "Deny", icon: "x", run: () => act(p, "decline") } : p.status === "working" ? { label: "Interrupt", icon: "stop", run: () => act(p, "interrupt") } : undefined}
      >
        <div className="card-row">
          <Avatar pane={p} morph />
          <div className="card-body">
            <div className="card-top">
              <span className="card-title" style={{ viewTransitionName: vtName("tt", p.id) } as CSSProperties}>
                {p.title}
              </span>
              <span className="time">{ago(p.updatedAt)}</span>
            </div>
            <div className="card-meta">
              <span className="status-text" data-status={p.status}>
                {p.status === "working" && <span className="orb" aria-hidden />}
                {STATUS_LABEL[p.status]}
              </span>
              <span className="sep">·</span>
              {p.machineName && (
                <>
                  <span className="machine">{p.machineName}</span>
                  <span className="sep">·</span>
                </>
              )}
              <span className="ws">{p.workspace}</span>
              {info && (
                <>
                  <span className="sep">·</span>
                  <span className="prov">{info.name}</span>
                </>
              )}
            </div>
            {pv && !approval && <p className="preview">{pv}</p>}
          </div>
        </div>
        {approval && <pre className="request-code">{p.request?.text}</pre>}
        {approval && (
          <div className="actions">
            <button className="btn primary" onClick={() => act(p, "approve")}>
              <Icon name="check" size={15} /> Approve
            </button>
            <button className="btn" onClick={() => open(p)}>
              Open
            </button>
            <button className="btn ghost danger" onClick={() => act(p, "decline")}>
              Deny
            </button>
          </div>
        )}
        {picks.length > 0 && (
          <div className="options compact">
            {picks.slice(0, 3).map((o) => (
              <button key={o.n} className="option" onClick={() => choose(p, o)}>
                <span className="opt-n">{o.n}</span>
                <span className="opt-body">
                  <b>{o.label}</b>
                </span>
              </button>
            ))}
            {picks.length > 3 && <span className="more">+{picks.length - 3} more in the thread</span>}
          </div>
        )}
      </SwipeCard>
    );
  };

  /** Requests stand alone as cards; everything else sits in one bordered list. */
  const group = (items: Pane[]) => {
    const cards = items.every((p) => p.status === "needs-input" || p.status === "error");
    return (
      <div className="group-list" data-kind={cards ? "cards" : "rows"}>
        {items.map(card)}
      </div>
    );
  };

  return (
    <div className="screen">
      <header className="bar">
        <span className="live-dot" data-state={link} aria-hidden />
        <div className="bar-title">
          <b>{desktop}</b>
          <span>{multi && link === "online" ? `${state.machines.length - away.length} of ${state.machines.length} connected` : link === "online" ? "Connected" : link === "away" ? (state.welcome?.server ?? activeIsServer() ? "Server offline" : "Desktop offline") : link === "offline" ? "Offline" : "Connecting…"}</span>
        </div>
        {state.panes.length > 0 && <span className="bar-chip num">{state.panes.length} active</span>}
      </header>
      <div className="ptr" ref={ptr} aria-hidden>
        <Icon name="refresh" size={16} />
      </div>
      <div className="scroll tabbed" ref={scroller}>
        <section className="hero">
          <h1>Agents</h1>
          <p>{summary(counts, state.panes.length)}</p>
        </section>
        {state.panes.length > 0 && (
          <form
            className="pick-search agent-search"
            role="search"
            onSubmit={(e) => {
              e.preventDefault();
              (document.activeElement as HTMLElement | null)?.blur();
            }}
          >
            <Icon name="search" size={15} />
            <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search workspaces, sessions, content…" aria-label="Search agents" inputMode="search" enterKeyHint="search" autoCapitalize="off" autoCorrect="off" spellCheck={false} />
            {query && (
              <button type="button" className="icon-btn" aria-label="Clear search" onClick={() => setQuery("")}>
                <Icon name="x" size={15} />
              </button>
            )}
          </form>
        )}
        <div className="filters">
          <Segmented<Filter>
            value={filter}
            onChange={setFilter}
            options={[
              { value: "all", label: "All", count: found.length },
              { value: "needs", label: "Needs you", count: foundCounts["needs-input"] + foundCounts.error, hot: true },
              { value: "working", label: "Working", count: foundCounts.working },
              { value: "done", label: "Done", count: foundCounts.done },
            ]}
          />
        </div>
        <ConnNotice state={state} onRetry={() => client.reconnect()} />
        {multi &&
          link === "online" &&
          failed.map((m) => (
            <div key={m.id} className="notice" data-tone="danger" role="status">
              <Icon name="x" />
              <p>
                {m.name}: {m.error}
              </p>
              {m.socket === "closed" && (
                <button className="btn sm" onClick={() => client.reconnect()}>
                  Retry
                </button>
              )}
            </div>
          ))}
        {multi && link === "online" && gone.length > 0 && (
          <div className="notice" role="status">
            <Icon name="laptop" />
            <p>
              {gone.map((m) => m.name).join(", ")} {gone.length === 1 ? "isn't" : "aren't"} connected: {gone.length === 1 ? "its" : "their"} agents show once {gone.length === 1 ? "it's" : "they're"} back.
            </p>
          </div>
        )}
        <div className="list" ref={list}>
          {filter === "all"
            ? GROUPS.map((g) => {
                const items = shown.filter((p) => g.statuses.includes(p.status));
                if (!items.length) return null;
                return (
                  <section key={g.title} className="list-group">
                    <h2 className="label" data-flip>
                      {g.title}
                      <span>{items.length}</span>
                    </h2>
                    {group(items)}
                  </section>
                );
              })
            : shown.length > 0 && group(shown)}
          {link === "online" && shown.length === 0 && (
            <div className="empty">
              <span className="empty-mark">
                <img src="./icons/logo.svg" alt="" />
              </span>
              <h3>{!state.panes.length ? "No agents running" : found.length ? "Nothing here" : "No matches"}</h3>
              <p className="sub">{state.panes.length ? (found.length ? "Try another filter." : "Try another search.") : canStart ? "Tap + below, or start one in winfer; it appears here live." : "Start one in winfer and it appears here, live."}</p>
            </div>
          )}
        </div>
      </div>
      {menu &&
        createPortal(
          <div className="sheet-wrap" onClick={() => setMenu(null)}>
            <div className="sheet" role="menu" onClick={(e) => e.stopPropagation()}>
              <header>
                <Avatar pane={menu} small />
                <div className="sheet-title">
                  <b>{menu.title}</b>
                  <span>
                    {menu.workspace} · {STATUS_LABEL[menu.status]}
                  </span>
                </div>
                <button className="icon-btn" aria-label="Close" onClick={() => setMenu(null)}>
                  <Icon name="x" size={18} />
                </button>
              </header>
              <div className="sheet-body">
                <button className="menu-item" role="menuitem" onClick={() => (setMenu(null), open(menu))}>
                  <Icon name="right" size={17} /> Open thread
                </button>
                <button className="menu-item danger" role="menuitem" disabled={link !== "online"} onClick={() => closeSession(menu)}>
                  <Icon name="x" size={17} /> {confirm ? "Stop the agent and close?" : "Close session"}
                </button>
              </div>
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
}
