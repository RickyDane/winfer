import { useRef, useState } from "react";
import type { DesktopClient } from "../client";
import { go, haptic, useClient } from "../hooks";
import { Icon, ProviderMark, linkState, providerInfo } from "./common";
import { Composer, Pill, PillIcon } from "./Composer";
import { Sheet } from "./Pickers";
import { Sky } from "./Sky";
import { Welcome } from "./Thread";

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

  const [sheet, setSheet] = useState<"ws" | "agent" | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const info = chosenProvider ? providerInfo(chosenProvider.id) : null;

  return (
    <div className="screen thread" data-empty>
      <Sky />
      <header className="pane-head">
        <button className="icon-btn" aria-label="Back" onClick={() => go("/")}>
          <Icon name="back" size={19} />
        </button>
        <div className="pane-title">
          <b>New session</b>
          <span className="pane-meta">
            {chosenWs ? <span className="chip-ws">{chosenWs.name}</span> : "Pick a workspace"}
            {chosenProvider && <span className="meta-item">{chosenProvider.name}</span>}
          </span>
        </div>
      </header>
      <div className="scroll">
        <div className="transcript">
          {workspaces.length === 0 ? (
            <div className="empty">
              <h3>{online ? "No workspaces" : "Desktop not connected"}</h3>
              <p className="sub">{online ? "Update winfer on the desktop to start sessions from your phone." : "Sessions start once your desktop is back online."}</p>
            </div>
          ) : (
            <Welcome
              status={[chosenProvider?.name, chosenWs?.name].filter(Boolean).join(" · ")}
              disabled={busy}
              onPick={(prompt) => {
                haptic(5);
                setText(prompt);
                input.current?.focus();
              }}
            />
          )}
        </div>
      </div>
      {workspaces.length > 0 && (
        <footer className="dock">
          {err && <div className="inline-error">{err}</div>}
          <Composer
            inputRef={input}
            value={text}
            onChange={setText}
            mode={busy ? "idle" : "send"}
            disabled={!online || busy || !chosenWs || !chosenProvider}
            onSend={() => void start()}
            placeholder={`Message ${chosenProvider?.name ?? "the agent"}…`}
            pills={
              <>
                <Pill
                  icon={<span style={{ color: info?.mark, display: "grid" }}>{chosenProvider ? <ProviderMark provider={chosenProvider.id} size={14} /> : <PillIcon name="sparkle" />}</span>}
                  label={chosenProvider?.name ?? "Agent"}
                  title="Agent"
                  onClick={() => (haptic(5), setSheet("agent"))}
                />
                <Pill icon={<PillIcon name="folder" />} label={chosenWs?.name ?? "Workspace"} title="Workspace" onClick={() => (haptic(5), setSheet("ws"))} />
                {busy && <span className="spinner" />}
              </>
            }
          />
        </footer>
      )}
      {sheet === "ws" && (
        <Sheet label="Workspace" title="Workspace" sub="Where the agent works" onClose={() => setSheet(null)}>
          <div className="pick-list scrolls">
            {workspaces.map((w) => (
              <button key={w.id} className="pick-row" data-on={w.id === chosenWs?.id} onClick={() => (haptic(5), setWs(w.id), setSheet(null))}>
                <span className="pick-icon">
                  <Icon name="folder" size={15} />
                </span>
                <span className="pick-body">
                  <b>{w.name}</b>
                  <small>
                    {sessions(state.panes.filter((p) => p.workspaceId === w.id).length)}
                    {w.active ? " · open on the desktop" : ""}
                  </small>
                </span>
                {w.id === chosenWs?.id && <Icon name="check" size={16} className="pick-check" />}
              </button>
            ))}
          </div>
        </Sheet>
      )}
      {sheet === "agent" && (
        <Sheet label="Agent" title="Agent" sub="Who answers" onClose={() => setSheet(null)}>
          <div className="pick-list scrolls">
            {state.providers.map((p) => (
              <button key={p.id} className="pick-row" data-on={p.id === chosenProvider?.id} onClick={() => (haptic(5), setProvider(p.id), setSheet(null))}>
                <span className="pick-icon mark" style={{ color: providerInfo(p.id)?.mark }}>
                  <ProviderMark provider={p.id} size={15} />
                </span>
                <span className="pick-body">
                  <b>{p.name}</b>
                  {p.id === state.defaultProvider && <small>Default</small>}
                </span>
                {p.id === chosenProvider?.id && <Icon name="check" size={16} className="pick-check" />}
              </button>
            ))}
          </div>
        </Sheet>
      )}
    </div>
  );
}
