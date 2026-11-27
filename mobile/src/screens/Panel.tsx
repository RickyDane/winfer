// The Panel tab: the desktop's side panel for a workspace — its browser, the Android mirror,
// terminals and files — each opening full screen.
import { useState } from "react";
import type { DesktopClient } from "../client";
import { go, haptic, useClient } from "../hooks";
import type { PanelInfo } from "../protocol";
import { ConnNotice, Icon, ProviderMark, linkState, type IconName } from "./common";
import { call, savedWorkspace, saveWorkspace, usePoll } from "./panelKit";

const MODULES: { id: "browser" | "android" | "terminal" | "files"; label: string; icon: IconName; text: string }[] = [
  { id: "browser", label: "Browser", icon: "globe", text: "The workspace's tabs, live" },
  { id: "android", label: "Android", icon: "phone", text: "Devices and emulators" },
  { id: "terminal", label: "Terminal", icon: "terminal", text: "The workspace's shell" },
  { id: "files", label: "Files", icon: "file", text: "Browse, read and edit" },
];

export function Panel({ client }: { client: DesktopClient }) {
  const state = useClient(client);
  const workspaces = state.workspaces;
  const [picked, setPicked] = useState(savedWorkspace);
  const ws = workspaces.find((w) => w.id === picked) ?? workspaces.find((w) => w.active) ?? workspaces[0];
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

  const open = (id: (typeof MODULES)[number]["id"]) => {
    if (!ws) return;
    haptic(6);
    const w = encodeURIComponent(ws.id);
    if (id === "android") go("/panel/android");
    else if (id === "terminal") go(`/panel/terminal/${w}/${encodeURIComponent(`side-terminal-${ws.id}`)}`);
    else go(`/panel/${id}/${w}`);
  };
  const terminals = info?.workspaceId === ws?.id ? info.terminals : [];

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
                {w.name}
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
                    <button key={t.id} className="row" disabled={!online} onClick={() => go(`/panel/terminal/${encodeURIComponent(ws.id)}/${encodeURIComponent(t.id)}`)}>
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
