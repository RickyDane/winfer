import { useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { go, haptic, useClient } from "../hooks";
import { Aurora, Icon, ProviderMark, linkState } from "./common";

const sessions = (n: number) => (n === 0 ? "No sessions" : n === 1 ? "1 session" : `${n} sessions`);

/** Starts an agent session in one of the desktop's workspaces, optionally with its first message. */
export function NewSession({ client, workspaceId }: { client: DesktopClient; workspaceId: string | null }) {
  const state = useClient(client);
  const workspaces = state.workspaces;
  const [ws, setWs] = useState<string | null>(workspaceId);
  const [provider, setProvider] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  // Until picked: the workspace asked for, else the one open on the desktop; the desktop's default agent.
  const chosenWs = workspaces.find((w) => w.id === ws) ?? workspaces.find((w) => w.active) ?? workspaces[0] ?? null;
  const chosenProvider = state.providers.find((p) => p.id === provider) ?? state.providers.find((p) => p.id === state.defaultProvider) ?? state.providers[0] ?? null;
  const online = linkState(state) === "online";

  async function start() {
    if (!chosenWs || !chosenProvider || busy) return;
    setBusy(true);
    setErr(null);
    haptic([8, 30, 8]);
    try {
      const reply = await client.request({ t: "pane.new", workspaceId: chosenWs.id, provider: chosenProvider.id, text: text.trim() || undefined });
      const id = reply.t === "ack" ? reply.paneId : undefined;
      // Back from the new thread goes to the agents, not to this form.
      if (id) location.replace(`#/pane/${encodeURIComponent(id)}`);
      else go("/");
    } catch (e) {
      setErr((e as Error).message);
      setBusy(false);
    }
  }

  return (
    <div className="screen">
      <Aurora />
      <header className="thread-head">
        <button className="icon-btn" aria-label="Back" onClick={() => go("/")}>
          <Icon name="back" />
        </button>
        <div className="thread-title">
          <b>New session</b>
          <span>{chosenWs ? `${chosenWs.name}${chosenProvider ? ` · ${chosenProvider.name}` : ""}` : "Pick a workspace"}</span>
        </div>
      </header>
      <div className="scroll">
        <main className="settings new-session">
          {workspaces.length === 0 && (
            <div className="empty">
              <h3>{online ? "No workspaces" : "Desktop not connected"}</h3>
              <p className="sub">{online ? "Update winfer on the desktop to start sessions from your phone." : "Sessions start once your desktop is back online."}</p>
            </div>
          )}
          {workspaces.length > 0 && (
            <section className="group" style={{ "--i": 0 } as CSSProperties}>
              <h2>Workspace</h2>
              <div className="group-card">
                {workspaces.map((w) => (
                  <button key={w.id} className="row" aria-pressed={w.id === chosenWs?.id} onClick={() => (haptic(5), setWs(w.id))}>
                    <span className="row-icon">
                      <Icon name="laptop" />
                    </span>
                    <span className="row-main">
                      <b>{w.name}</b>
                      <span>
                        {sessions(state.panes.filter((p) => p.workspaceId === w.id).length)}
                        {w.active ? " · open on the desktop" : ""}
                      </span>
                    </span>
                    {w.id === chosenWs?.id && <Icon name="check" className="check" />}
                  </button>
                ))}
              </div>
            </section>
          )}
          {state.providers.length > 0 && (
            <section className="group" style={{ "--i": 1 } as CSSProperties}>
              <h2>Agent</h2>
              <div className="group-card">
                {state.providers.map((p) => (
                  <button key={p.id} className="row" aria-pressed={p.id === chosenProvider?.id} onClick={() => (haptic(5), setProvider(p.id))}>
                    <ProviderMark provider={p.id} size={32} />
                    <span className="row-main">
                      <b>{p.name}</b>
                      {p.id === state.defaultProvider && <span>Default</span>}
                    </span>
                    {p.id === chosenProvider?.id && <Icon name="check" className="check" />}
                  </button>
                ))}
              </div>
            </section>
          )}
          {workspaces.length > 0 && (
            <section className="group" style={{ "--i": 2 } as CSSProperties}>
              <h2>First message</h2>
              <div className="group-card">
                <textarea
                  className="new-prompt"
                  rows={4}
                  value={text}
                  placeholder="What should the agent do? (optional)"
                  onChange={(e) => setText(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void start();
                  }}
                />
              </div>
              <p className="group-foot">Leave it empty to open the agent and write to it from its thread.</p>
            </section>
          )}
        </main>
      </div>
      {workspaces.length > 0 && (
        <footer className="dock">
          {err && <div className="inline-error">{err}</div>}
          <button className="btn primary block" disabled={!online || busy || !chosenWs || !chosenProvider} onClick={() => void start()}>
            <Icon name="plus" size={18} /> {busy ? "Starting…" : text.trim() ? "Start session" : "Open session"}
          </button>
        </footer>
      )}
    </div>
  );
}
