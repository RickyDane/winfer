import { createPortal } from "react-dom";
import { useCallback, useMemo, useRef, useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { ago, go, haptic, useClient, useTick, vtName } from "../hooks";
import { isFreeText, sortPanes, type AgentStatus, type Pane, type RequestOption } from "../protocol";
import { Avatar, ConnNotice, Icon, STATUS_LABEL, linkState, providerInfo } from "./common";
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

  const panes = sortPanes(state.panes);
  const counts = useMemo(() => {
    const c: Record<AgentStatus, number> = { "needs-input": 0, working: 0, done: 0, error: 0, idle: 0 };
    for (const p of state.panes) c[p.status]++;
    return c;
  }, [state.panes]);
  const link = linkState(state);
  const desktop = state.welcome?.desktop ?? client.name;
  const shown = panes.filter(MATCH[filter]);
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
  // The new-session screen picks the workspace, so the inbox needs only one way in.
  const startNew = () => {
    haptic(6);
    go("/new");
  };
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
          <span>{link === "online" ? "Connected" : link === "away" ? "Desktop offline" : link === "offline" ? "Offline" : "Connecting…"}</span>
        </div>
        {canStart && (
          <button className="btn primary sm" disabled={link !== "online"} onClick={startNew}>
            <Icon name="plus" size={15} /> New
          </button>
        )}
      </header>
      <div className="ptr" ref={ptr} aria-hidden>
        <Icon name="refresh" size={16} />
      </div>
      <div className="scroll tabbed" ref={scroller}>
        <section className="hero">
          <h1>Agents</h1>
          <p>{summary(counts, state.panes.length)}</p>
        </section>
        <div className="filters">
          <Segmented<Filter>
            value={filter}
            onChange={setFilter}
            options={[
              { value: "all", label: "All", count: state.panes.length },
              { value: "needs", label: "Needs you", count: counts["needs-input"] + counts.error, hot: true },
              { value: "working", label: "Working", count: counts.working },
              { value: "done", label: "Done", count: counts.done },
            ]}
          />
        </div>
        <ConnNotice state={state} onRetry={() => client.reconnect()} />
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
              <h3>{state.panes.length ? "Nothing here" : "No agents running"}</h3>
              <p className="sub">{state.panes.length ? "Try another filter." : canStart ? "Start one with New, or in winfer; it appears here live." : "Start one in winfer and it appears here, live."}</p>
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
