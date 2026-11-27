// The Panel tab: the desktop's side panel for a workspace — its browser, the Android mirror,
// terminals and files — each opening full screen.
import { useState } from "react";
import type { ClientState, DesktopClient } from "../client";
import { go, haptic, setPanelBack, useClient } from "../hooks";
import { forMachine } from "../multi";
import type { Pane, PanelInfo, Workspace } from "../protocol";
import { ConnNotice, Icon, ProviderMark, linkState, type IconName } from "./common";
import { Sheet } from "./Pickers";
import { call, savedWorkspace, saveWorkspace, usePoll } from "./panelKit";

type ModuleId = "browser" | "android" | "terminal" | "files";
const MODULES: { id: ModuleId; label: string; icon: IconName; text: string }[] = [
  { id: "browser", label: "Browser", icon: "globe", text: "The workspace's tabs, live" },
  { id: "android", label: "Android", icon: "phone", text: "Devices and emulators" },
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
  const ws = workspaces.find((w) => w.id === picked) ?? workspaces.find((w) => w.active) ?? workspaces[0];
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
          <span>{ws ? `${ws.name} on ${state.welcome?.desktop ?? "the desktop"}` : "The desktop's side panel"}</span>
        </div>
      </header>
      <div className="scroll tabbed">
        <ConnNotice state={state} onRetry={() => client.reconnect()} />
        {workspaces.length > 1 && (
          <div className="panel-ws chips">
            {workspaces.map((w) => (
              <button
                key={w.id}
                className="chip"
                aria-pressed={w.id === ws?.id}
                onClick={() => {
                  setPicked(w.id);
                  saveWorkspace(w.id);
                }}
              >
                {w.machineName ? `${w.name} · ${w.machineName}` : w.name}
              </button>
            ))}
          </div>
        )}
        {state.welcome?.server ? (
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
