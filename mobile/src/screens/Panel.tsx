// The Panel tab: the desktop's side panel for a workspace — its browser, the device mirror,
// terminals and files — each opening full screen.
import { useState } from "react";
import type { ClientState, DesktopClient } from "../client";
import { go, haptic, setPanelBack, useClient } from "../hooks";
import { forMachine } from "../multi";
import type { Pane, PanelInfo, Workspace, WorkspaceTabs } from "../protocol";
import { ConnNotice, Icon, ProviderMark, linkState, type IconName } from "./common";
import { relative } from "./Files";
import { Sheet } from "./Pickers";
import { ALL, WorkspaceCombo } from "./WorkspaceCombo";
import { call, savedWorkspace, saveWorkspace, usePoll } from "./panelKit";

type ModuleId = "browser" | "android" | "terminal" | "files";
const MODULES: { id: ModuleId; label: string; icon: IconName; text: string }[] = [
  { id: "browser", label: "Browser", icon: "globe", text: "The workspace's tabs, live" },
  { id: "android", label: "Devices", icon: "phone", text: "Android and iOS simulators" },
  { id: "terminal", label: "Terminal", icon: "terminal", text: "The workspace's shell" },
  { id: "files", label: "Files", icon: "file", text: "Browse, read and edit" },
];

/** Opens a side-panel module of workspace `wsId`; its Back returns to `from`. */
function openModule(wsId: string, id: ModuleId, from: string) {
  haptic(6);
  setPanelBack(from);
  const w = encodeURIComponent(wsId);
  if (id === "terminal") go(`/panel/terminal/${w}/${encodeURIComponent(`side-terminal-${wsId}`)}`);
  // Android too: the workspace picks the machine whose devices show.
  else go(`/panel/${id}/${w}`);
}

function openTerminal(wsId: string, termId: string, from: string) {
  haptic(6);
  setPanelBack(from);
  go(`/panel/terminal/${encodeURIComponent(wsId)}/${encodeURIComponent(termId)}`);
}

/** A pane's workspace (older desktops send only its name). */
export function paneWorkspace(state: ClientState, pane: Pane): Workspace | undefined {
  return state.workspaces.find((w) => w.id === pane.workspaceId) ?? state.workspaces.find((w) => w.name === pane.workspace && w.machine === pane.machine);
}

/** The panel can show this pane's workspace: a desktop's (servers have no side panel). */
export function hasPanel(state: ClientState, pane: Pane): boolean {
  return !!paneWorkspace(state, pane) && !forMachine(state, pane.machine).welcome?.server;
}

/** The side panel from a chat: the agent's workspace's modules and terminals; Back returns to the chat. */
export function PanelSheet({ client, pane, onClose }: { client: DesktopClient; pane: Pane; onClose: () => void }) {
  const all = useClient(client);
  const ws = paneWorkspace(all, pane);
  const online = linkState(forMachine(all, pane.machine)) === "online";
  const [info, setInfo] = useState<PanelInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  usePoll(
    () => {
      if (!ws || !online) return;
      call<PanelInfo>(client, { t: "panel.get", workspaceId: ws.id })
        .then((i) => (setInfo(i), setError(null)))
        .catch((e: Error) => setError(e.message));
    },
    5000,
    [ws?.id, online],
  );
  if (!ws) return null;
  const from = `/pane/${encodeURIComponent(pane.id)}`;
  const leave = (fn: () => void) => {
    saveWorkspace(ws.id);
    onClose();
    fn();
  };
  const terminals = info && info.workspaceId === ws.id ? info.terminals.filter((t) => !t.side) : [];
  // The agent's own terminal first (a CLI agent's pane is a terminal on the desktop).
  terminals.sort((a, b) => Number(b.id === pane.id) - Number(a.id === pane.id));
  return (
    <Sheet label="Side panel" title="Side panel" sub={ws.machineName ? `${ws.name} · ${ws.machineName}` : ws.name} onClose={onClose}>
      <div className="sheet-body panel-sheet">
        <div className="panel-grid">
          {MODULES.map((m) => (
            <button key={m.id} className="panel-tile" disabled={!online} onClick={() => leave(() => openModule(ws.id, m.id, from))}>
              <span className="panel-tile-icon">
                <Icon name={m.icon} size={20} />
              </span>
              <b>{m.label}</b>
              <span>{m.text}</span>
            </button>
          ))}
        </div>
        {error && <p className="inline-error">{error}</p>}
        {terminals.length > 0 && (
          <section className="group">
            <h2>Terminals</h2>
            <div className="group-card">
              {terminals.map((t) => (
                <button key={t.id} className="row" disabled={!online} onClick={() => leave(() => openTerminal(ws.id, t.id, from))}>
                  <span className="row-icon">
                    <ProviderMark provider={t.provider} size={15} />
                  </span>
                  <span className="row-main">
                    <b>{t.id === pane.id ? "This agent's terminal" : t.title}</b>
                    <span>{t.running ? "Running" : "Not started"}</span>
                  </span>
                  <Icon name="right" size={16} />
                </button>
              ))}
            </div>
          </section>
        )}
      </div>
    </Sheet>
  );
}

export function Panel({ client }: { client: DesktopClient }) {
  const all = useClient(client);
  const workspaces = all.workspaces;
  const [picked, setPicked] = useState(savedWorkspace);
  // "All workspaces", like the desktop's Overview (only with more than one).
  const showAll = picked === ALL && workspaces.length > 1;
  const ws = showAll ? undefined : (workspaces.find((w) => w.id === picked) ?? workspaces.find((w) => w.active) ?? workspaces[0]);
  // With several machines: the picked workspace's machine's link and kind.
  const state = forMachine(all, ws?.machine);
  const [info, setInfo] = useState<PanelInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const online = linkState(state) === "online";

  usePoll(
    () => {
      if (!ws || !online) return;
      call<PanelInfo>(client, { t: "panel.get", workspaceId: ws.id })
        .then((i) => {
          setInfo(i);
          setError(null);
        })
        .catch((e: Error) => setError(e.message));
    },
    5000,
    [ws?.id, online],
  );

  const open = (id: ModuleId) => ws && openModule(ws.id, id, "/panel");
  const terminals = info && info.workspaceId === ws?.id ? info.terminals : [];

  return (
    <div className="screen">
      <header className="bar">
        <div className="bar-title">
          <b>Panel</b>
          <span>{showAll ? `All workspaces${all.machines.length > 1 ? `, ${all.machines.length} machines` : ` on ${state.welcome?.desktop ?? "the desktop"}`}` : ws ? `${ws.name} on ${state.welcome?.desktop ?? "the desktop"}` : "The desktop's side panel"}</span>
        </div>
      </header>
      <div className="scroll tabbed">
        <ConnNotice state={state} onRetry={() => client.reconnect()} />
        {workspaces.length > 1 && (
          <div className="panel-ws">
            <WorkspaceCombo
              workspaces={workspaces}
              value={showAll ? ALL : (ws?.id ?? "")}
              onChange={(id) => {
                setPicked(id);
                saveWorkspace(id);
              }}
            />
          </div>
        )}
        {showAll ? (
          <AllWorkspaces client={client} />
        ) : state.welcome?.server ? (
          <div className="empty">
            <span className="empty-mark">
              <Icon name="panel" />
            </span>
            <h3>On desktops for now</h3>
            <p className="sub">The browser, Android mirror, terminals and files come from a paired desktop's side panel. Pair this phone with your desktop to use them here.</p>
          </div>
        ) : !ws ? (
          <div className="empty">
            <span className="empty-mark">
              <Icon name="panel" />
            </span>
            <h3>No workspaces</h3>
            <p className="sub">Open a workspace on the desktop to use its browser, terminal and files here.</p>
          </div>
        ) : (
          <main className="panel-hub">
            <div className="panel-grid">
              {MODULES.map((m) => (
                <button key={m.id} className="panel-tile" disabled={!online} onClick={() => open(m.id)}>
                  <span className="panel-tile-icon">
                    <Icon name={m.icon} size={20} />
                  </span>
                  <b>{m.label}</b>
                  <span>{m.text}</span>
                </button>
              ))}
            </div>
            {error && <p className="inline-error">{error}</p>}
            {terminals.length > 0 && (
              <section className="group">
                <h2>Terminals</h2>
                <div className="group-card">
                  {terminals.map((t) => (
                    <button key={t.id} className="row" disabled={!online} onClick={() => openTerminal(ws.id, t.id, "/panel")}>
                      <span className="row-icon">{t.side ? <Icon name="terminal" size={16} /> : <ProviderMark provider={t.provider} size={15} />}</span>
                      <span className="row-main">
                        <b>{t.side ? "Side terminal" : t.title}</b>
                        <span>{t.side ? info?.root : t.running ? "Running" : "Not started"}</span>
                      </span>
                      <Icon name="right" size={16} />
                    </button>
                  ))}
                </div>
              </section>
            )}
          </main>
        )}
      </div>
    </div>
  );
}

const parentDir = (p: string) => p.replace(/\/+$/, "").replace(/\/[^/]*$/, "") || "/";
const fileName = (p: string) => p.replace(/\/+$/, "").split("/").pop() || p;
const hostOf = (url: string) => {
  try {
    const u = new URL(url);
    return u.protocol === "about:" ? url : u.host + (u.pathname === "/" ? "" : u.pathname);
  } catch {
    return url;
  }
};

type Docs = { root: string; docs: string[]; groups?: { id: string; name: string; docs: string[] }[] };
/** One machine's overview: its workspaces' browser tabs and the files open in its viewer. */
type MachineView = { anchor: Workspace; tabs: WorkspaceTabs[]; docs: Docs | null };

/** "All workspaces": every workspace's browser tabs, open files and terminals, like the desktop's Overview. */
function AllWorkspaces({ client }: { client: DesktopClient }) {
  const all = useClient(client);
  // Desktops only (servers have no side panel), and only those online.
  const live = all.workspaces.filter((w) => {
    const m = forMachine(all, w.machine);
    return !m.welcome?.server && linkState(m) === "online";
  });
  // Each machine answers for all its workspaces through one of them.
  const anchors = live.filter((w, i) => live.findIndex((x) => x.machine === w.machine) === i);
  const [views, setViews] = useState<Record<string, MachineView>>({});
  const [infos, setInfos] = useState<Record<string, PanelInfo>>({});
  const [error, setError] = useState<string | null>(null);
  const key = live.map((w) => w.id).join(",");

  usePoll(
    () => {
      for (const anchor of anchors) {
        const m = anchor.machine ?? "";
        // Older desktops can't list every workspace's tabs: no tabs from them.
        const tabs = call<{ all: WorkspaceTabs[] }>(client, { t: "browser.call", op: "tabs", workspaceId: anchor.id }).then((v) => v.all ?? [], () => []);
        const docs = call<Docs>(client, { t: "files.call", op: "docs", workspaceId: anchor.id }).catch(() => null);
        void Promise.all([tabs, docs]).then(([t, d]) => setViews((v) => ({ ...v, [m]: { anchor, tabs: t, docs: d } })));
      }
      for (const w of live) {
        call<PanelInfo>(client, { t: "panel.get", workspaceId: w.id })
          .then((i) => (setInfos((x) => ({ ...x, [w.id]: i })), setError(null)))
          .catch((e: Error) => setError(e.message));
      }
    },
    5000,
    [key],
  );

  const several = all.machines.length > 1;
  const wsOf = (id: string) => all.workspaces.find((w) => w.id === id);
  const where = (w: Workspace | undefined, name?: string) => (w ? (several && w.machineName ? `${w.name} · ${w.machineName}` : w.name) : (name ?? ""));
  const machines = anchors.map((a) => views[a.machine ?? ""]).filter(Boolean);
  const tabs = machines.flatMap((v) => v.tabs.flatMap((g) => g.tabs.map((t) => ({ ws: g.workspaceId, name: g.name, tab: t, shown: t.id === g.active }))));
  const files = machines.flatMap((v) =>
    (v.docs?.groups ?? (v.docs ? [{ id: "", name: v.anchor.name, docs: v.docs.docs }] : [])).flatMap((g) =>
      g.docs.map((d) => ({ anchor: v.anchor, path: d, group: several && v.anchor.machineName ? `${g.name} · ${v.anchor.machineName}` : g.name, root: v.docs?.root ?? "" })),
    ),
  );
  const terminals = live.flatMap((w) => (infos[w.id]?.terminals ?? []).filter((t) => !t.side || t.running).map((t) => ({ w, t })));

  const openTab = (wsId: string, tab: string) => {
    haptic(6);
    setPanelBack("/panel");
    void call(client, { t: "browser.call", op: "select", workspaceId: wsId, tab })
      .catch(() => {})
      .then(() => go(`/panel/browser/${encodeURIComponent(wsId)}`));
  };
  const openFile = (anchor: Workspace, path: string) => {
    haptic(6);
    setPanelBack("/panel");
    go(`/panel/files/${encodeURIComponent(anchor.id)}/${encodeURIComponent(path)}`);
  };

  if (!live.length)
    return (
      <div className="empty">
        <span className="empty-mark">
          <Icon name="panel" />
        </span>
        <h3>No desktop online</h3>
        <p className="sub">The browser, files and terminals show here once a paired desktop is back.</p>
      </div>
    );
  return (
    <main className="panel-hub panel-all">
      <section className="group">
        <h2>Devices</h2>
        <div className="group-card">
          {anchors.map((a) => (
            <button key={a.id} className="row" onClick={() => openModule(a.id, "android", "/panel")}>
              <span className="row-icon">
                <Icon name="phone" size={16} />
              </span>
              <span className="row-main">
                <b>Emulators and simulators</b>
                <span>{several && a.machineName ? `On ${a.machineName}` : "On the desktop"}</span>
              </span>
              <Icon name="right" size={16} />
            </button>
          ))}
        </div>
      </section>
      <section className="group">
        <h2>Browser tabs</h2>
        {tabs.length ? (
          <div className="group-card">
            {tabs.map(({ ws: id, name, tab, shown }) => (
              <button key={id + tab.id} className="row" onClick={() => openTab(id, tab.id)}>
                <span className="row-icon">{tab.favicon ? <img className="row-favicon" src={tab.favicon} alt="" /> : <Icon name="globe" size={16} />}</span>
                <span className="row-main">
                  <b>{tab.title || hostOf(tab.url) || "New tab"}</b>
                  <span>
                    {where(wsOf(id), name)} · {hostOf(tab.url)}
                    {shown ? " · shown" : ""}
                  </span>
                </span>
                <Icon name="right" size={16} />
              </button>
            ))}
          </div>
        ) : (
          <p className="group-foot">{machines.length ? "No tabs open in any workspace." : "Loading…"}</p>
        )}
      </section>
      <section className="group">
        <h2>Open files</h2>
        {files.length ? (
          <div className="group-card">
            {files.map((f) => (
              <button key={f.anchor.id + f.path} className="row" onClick={() => openFile(f.anchor, f.path)}>
                <span className="row-icon">
                  <Icon name="file" size={16} />
                </span>
                <span className="row-main">
                  <b>{fileName(f.path)}</b>
                  <span>
                    {f.group} · {relative(parentDir(f.path), f.root)}
                  </span>
                </span>
                <Icon name="right" size={16} />
              </button>
            ))}
          </div>
        ) : (
          <p className="group-foot">{machines.length ? "No files open in the desktop's viewer." : "Loading…"}</p>
        )}
      </section>
      <section className="group">
        <h2>Terminals</h2>
        {terminals.length ? (
          <div className="group-card">
            {terminals.map(({ w, t }) => (
              <button key={w.id + t.id} className="row" onClick={() => openTerminal(w.id, t.id, "/panel")}>
                <span className="row-icon">{t.side ? <Icon name="terminal" size={16} /> : <ProviderMark provider={t.provider} size={15} />}</span>
                <span className="row-main">
                  <b>{t.side ? "Side terminal" : t.title}</b>
                  <span>
                    {where(w)} · {t.running ? "Running" : "Not started"}
                  </span>
                </span>
                <Icon name="right" size={16} />
              </button>
            ))}
          </div>
        ) : (
          <p className="group-foot">{Object.keys(infos).length ? "No terminals in any workspace." : "Loading…"}</p>
        )}
        {error && <p className="inline-error">{error}</p>}
      </section>
    </main>
  );
}
