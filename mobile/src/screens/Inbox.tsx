import { useCallback, useMemo, useRef, useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { ago, go, haptic, useClient, useTick, vtName } from "../hooks";
import { isFreeText, sortPanes, type AgentStatus, type Pane, type RequestOption } from "../protocol";
import { Aurora, Avatar, ConnNotice, Icon, STATUS_LABEL, linkState, providerInfo } from "./common";
import { Segmented, StatusRing, SwipeCard, useCollapse, useFlip, usePullToRefresh } from "./widgets";

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

function headline(counts: Record<AgentStatus, number>, total: number) {
  const needs = counts["needs-input"];
  if (needs) return <>{needs === 1 ? "An agent" : `${needs} agents`} <em>{needs === 1 ? "needs" : "need"} you</em></>;
  if (counts.error) return <>{counts.error === 1 ? "An agent" : `${counts.error} agents`} <em>hit an error</em></>;
  if (counts.working) return <>{counts.working === 1 ? "One agent" : `${counts.working} agents`} <em className="calm">at work</em></>;
  if (total) return <>All quiet. <em className="calm">Nothing waiting.</em></>;
  return <>No agents <em className="calm">yet</em></>;
}

function preview(p: Pane): { text: string; mono: boolean } | null {
  if (p.request?.kind === "approval") return { text: p.request.text.split("\n")[0], mono: true };
  if (p.request) return { text: p.request.text, mono: false };
  const text = p.status === "done" ? (p.reply ?? p.message) : p.message;
  return text ? { text: text.replace(/[#*`_>]/g, "").trim(), mono: false } : null;
}

export function Inbox({ client }: { client: DesktopClient }) {
  const state = useClient(client);
  useTick(30_000);
  const [filter, setFilter] = useState<Filter>("all");
  const scroller = useRef<HTMLDivElement>(null);
  const screen = useRef<HTMLDivElement>(null);
  const ptr = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const onScroll = useCollapse(scroller, screen, 70);

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
  let i = 0;
  const card = (p: Pane) => {
    const pv = preview(p);
    const approval = p.request?.kind === "approval";
    const info = providerInfo(p.provider);
    return (
      <SwipeCard
        key={p.id}
        index={i++}
        status={p.status}
        onTap={() => open(p)}
        right={approval ? { label: "Approve", icon: "check", run: () => act(p, "approve") } : undefined}
        left={approval ? { label: "Decline", icon: "x", run: () => act(p, "decline") } : p.status === "working" ? { label: "Interrupt", icon: "stop", run: () => act(p, "interrupt") } : undefined}
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
                {STATUS_LABEL[p.status]}
              </span>
              <span className="sep">•</span>
              <span className="ws">{p.workspace}</span>
              {info && (
                <>
                  <span className="sep">•</span>
                  <span className="prov">{info.name}</span>
                </>
              )}
            </div>
          </div>
        </div>
        {pv && <p className={`preview${pv.mono ? " mono" : ""}`}>{pv.text}</p>}
        {approval && (
          <div className="card-actions">
            <button className="btn sm approve" onClick={() => act(p, "approve")}>
              <Icon name="check" size={16} /> Approve
            </button>
            <button className="btn sm" onClick={() => act(p, "decline")}>
              Decline
            </button>
            <span className="hint">
              swipe <Icon name="right" size={12} />
            </span>
          </div>
        )}
        {p.request?.kind === "question" && !!p.request.options?.length && (
          <div className="card-options">
            {p.request.options
              .filter((o) => !isFreeText(o))
              .slice(0, 3)
              .map((o) => (
                <button key={o.n} className="card-option" onClick={() => choose(p, o)}>
                  <span className="opt-n">{o.n}</span>
                  {o.label}
                </button>
              ))}
            {p.request.options.filter((o) => !isFreeText(o)).length > 3 && <span className="hint">+{p.request.options.filter((o) => !isFreeText(o)).length - 3} more</span>}
          </div>
        )}
        {p.status === "working" && <div className="progress" />}
      </SwipeCard>
    );
  };

  return (
    <div className="screen" ref={screen}>
      <Aurora mood={counts["needs-input"] ? "needs" : counts.error ? "error" : "calm"} />
      <header className="topbar">
        <div className="topbar-title">{desktop}</div>
      </header>
      <div className="ptr" ref={ptr} aria-hidden>
        <Icon name="refresh" size={18} />
      </div>
      <div className="scroll" ref={scroller} onScroll={onScroll}>
        <section className="hero">
          <div className="hero-text">
            <div className="eyebrow">
              <span className="live-dot" data-state={link} />
              <span>{link === "online" ? desktop : link === "away" ? `${desktop} · offline` : "Connecting…"}</span>
            </div>
            <h1 className="headline">{headline(counts, state.panes.length)}</h1>
          </div>
          <StatusRing counts={counts} total={state.panes.length} label={counts["needs-input"] ? "waiting" : "agents"} />
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
                return [
                  <div key={`g-${g.title}`} className="group-label" data-flip>
                    {g.title}
                  </div>,
                  ...items.map(card),
                ];
              })
            : shown.map(card)}
          {link === "online" && shown.length === 0 && (
            <div className="empty">
              <div className="orbit-art">
                <i />
                <i />
                <img src="./icons/logo.svg" alt="" />
              </div>
              <h3>{state.panes.length ? "Nothing here" : "No agents running"}</h3>
              <p className="sub">{state.panes.length ? "Try another filter." : "Start one in winfer and it appears here, live."}</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
