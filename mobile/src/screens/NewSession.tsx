import { useEffect, useRef, useState } from "react";
import type { DesktopClient } from "../client";
import { firstMessages, go, haptic, useClient } from "../hooks";
import type { ModelOption } from "../protocol";
import { Icon, ProviderMark, linkState, providerInfo } from "./common";
import { activeIsServer } from "../pairing";
import { forMachine } from "../multi";
import { Composer, Pill, PillIcon } from "./Composer";
import { EffortIcon, EffortListSheet, ModelListSheet, Sheet, backendOf, effortLabel } from "./Pickers";
import { Sky } from "./Sky";
import { Welcome } from "./Thread";

const sessions = (n: number) => (n === 0 ? "No sessions" : n === 1 ? "1 session" : `${n} sessions`);

interface ProviderCatalog {
  models: ModelOption[];
  defaultModel: string | null;
  defaultEffort: string | null;
  defaultEfforts: string[];
}

/** Starts an agent session in one of the desktop's workspaces, with optional model, effort, and first message. */
export function NewSession({ client, workspaceId }: { client: DesktopClient; workspaceId: string | null }) {
  const all = useClient(client);
  const workspaces = all.workspaces;
  const [ws, setWs] = useState<string | null>(workspaceId);
  const [provider, setProvider] = useState<string | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [effort, setEffort] = useState<string | null>(null);
  const [catalogs, setCatalogs] = useState<Record<string, ProviderCatalog | "loading">>({});
  const requested = useRef(new Set<string>());
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  // Until picked: the workspace asked for, else the one open on the desktop; the desktop's default agent.
  // With several machines, an answering machine's workspace first: the primary's open one, any open one, any.
  const up = (w: (typeof workspaces)[number]) => linkState(forMachine(all, w.machine)) === "online";
  const isPrimary = (w: (typeof workspaces)[number]) => !w.machine || !!all.machines.find((m) => m.id === w.machine)?.primary;
  const chosenWs =
    workspaces.find((w) => w.id === ws) ??
    workspaces.find((w) => w.active && isPrimary(w) && up(w)) ??
    workspaces.find((w) => w.active && up(w)) ??
    workspaces.find(up) ??
    workspaces.find((w) => w.active && isPrimary(w)) ??
    workspaces.find((w) => w.active) ??
    workspaces[0] ??
    null;
  // With several machines: the agents, link and models of the workspace's own machine.
  const machine = chosenWs?.machine;
  const state = forMachine(all, machine);
  const chosenProvider = state.providers.find((p) => p.id === provider) ?? state.providers.find((p) => p.id === state.defaultProvider) ?? state.providers[0] ?? null;
  const online = linkState(state) === "online";
  const providerId = chosenProvider?.id ?? null;
  const catalogKey = providerId ? `${machine ?? ""}/${providerId}` : null;

  // Fetch available models & effort defaults for the active provider (cached per provider).
  useEffect(() => {
    if (!providerId || !catalogKey || !online || requested.current.has(catalogKey)) return;
    requested.current.add(catalogKey);
    setCatalogs((c) => ({ ...c, [catalogKey]: "loading" }));
    client
      .request({ t: "models.get", provider: providerId }, machine)
      .then((r) => {
        const entry: ProviderCatalog =
          r.t === "models"
            ? { models: r.models, defaultModel: r.defaultModel ?? null, defaultEffort: r.defaultEffort ?? null, defaultEfforts: r.defaultEfforts ?? [] }
            : { models: [], defaultModel: null, defaultEffort: null, defaultEfforts: [] };
        setCatalogs((c) => ({ ...c, [catalogKey]: entry }));
      })
      .catch(() => {
        requested.current.delete(catalogKey);
        setCatalogs((c) => ({ ...c, [catalogKey]: { models: [], defaultModel: null, defaultEffort: null, defaultEfforts: [] } }));
      });
  }, [client, providerId, catalogKey, machine, online]);

  const rawCatalog = catalogKey ? catalogs[catalogKey] : undefined;
  const loadingModels = rawCatalog === "loading";
  const catalog = rawCatalog && rawCatalog !== "loading" ? rawCatalog : null;
  const models = catalog?.models ?? [];

  const effectiveModelId = model ?? catalog?.defaultModel ?? null;
  const effectiveModel = effectiveModelId ? models.find((m) => m.id === effectiveModelId) ?? null : null;
  const modelLabel = effectiveModel?.name ?? effectiveModelId ?? "Default model";
  const defaultModelName = catalog?.defaultModel ? (models.find((m) => m.id === catalog.defaultModel)?.name ?? catalog.defaultModel) : "Provider default";

  const availableEfforts = effectiveModel ? effectiveModel.efforts : (catalog?.defaultEfforts ?? []);
  const activeEffort =
    effort && availableEfforts.includes(effort)
      ? effort
      : catalog?.defaultEffort && availableEfforts.includes(catalog.defaultEffort)
        ? catalog.defaultEffort
        : "default";

  async function start() {
    if (!chosenWs || !chosenProvider || busy) return;
    setBusy(true);
    setErr(null);
    haptic([8, 30, 8]);
    try {
      const first = text.trim();
      const explicitEffort = effort && effort !== "default" && availableEfforts.includes(effort) ? effort : undefined;
      const reply = await client.request({
        t: "pane.new",
        workspaceId: chosenWs.id,
        provider: chosenProvider.id,
        text: first || undefined,
        model: model ?? undefined,
        effort: explicitEffort,
      });
      const id = reply.t === "ack" ? reply.paneId : undefined;
      // The thread replaces this screen in place (same page, no history entry), showing the message right away.
      if (id && first) firstMessages.set(id, first);
      if (id) location.replace(`#/pane/${encodeURIComponent(id)}`);
      else go("/");
    } catch (e) {
      setErr((e as Error).message);
      setBusy(false);
    }
  }

  const [sheet, setSheet] = useState<"ws" | "agent" | "model" | "effort" | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const info = chosenProvider ? providerInfo(chosenProvider.id) : null;
  const modelBackend = effectiveModelId ? backendOf(effectiveModelId) : null;
  const modelIconProvider = modelBackend?.id ?? (chosenProvider?.id !== "winfer" ? chosenProvider?.id : null) ?? null;

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
            {chosenWs?.machineName && <span className="meta-item">{chosenWs.machineName}</span>}
            {chosenWs ? <span className="chip-ws">{chosenWs.name}</span> : "Pick a workspace"}
            {chosenProvider && <span className="meta-item">{chosenProvider.name}</span>}
            {models.length > 0 && effectiveModel && <span className="meta-item">· {effectiveModel.name}</span>}
          </span>
        </div>
      </header>
      <div className="scroll">
        <div className="transcript">
          {workspaces.length === 0 ? (
            <div className="empty">
              <h3>{online ? "No workspaces" : "Desktop not connected"}</h3>
              <p className="sub">{online
                  ? (state.welcome?.server ?? activeIsServer()) ? "Update winferd on the server to start sessions from your phone." : "Update winfer on the desktop to start sessions from your phone."
                  : `Sessions start once your ${(state.welcome?.server ?? activeIsServer()) ? "server" : "desktop"} is back online.`}</p>
            </div>
          ) : (
            <Welcome
              server={state.welcome?.server}
              status={[chosenProvider?.name, effectiveModel?.name, availableEfforts.length > 1 && activeEffort !== "default" ? effortLabel(activeEffort) : null, chosenWs?.name].filter(Boolean).join(" · ")}
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
                {(models.length > 0 || loadingModels) && (
                  <Pill
                    icon={
                      loadingModels ? (
                        <span className="spinner" />
                      ) : modelIconProvider ? (
                        <span style={{ color: providerInfo(modelIconProvider)?.mark, display: "grid" }}>
                          <ProviderMark provider={modelIconProvider} size={14} />
                        </span>
                      ) : (
                        <PillIcon name="sparkle" />
                      )
                    }
                    label={loadingModels ? "Models…" : modelLabel}
                    title="Model"
                    onClick={() => (haptic(5), setSheet("model"))}
                  />
                )}
                {availableEfforts.length > 1 && (
                  <Pill
                    icon={<EffortIcon effort={activeEffort} size={14} />}
                    label={effortLabel(activeEffort)}
                    title="Reasoning effort"
                    onClick={() => (haptic(5), setSheet("effort"))}
                  />
                )}
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
              <button key={w.id} className="pick-row" data-on={w.id === chosenWs?.id} onClick={() => (haptic(5), w.machine !== machine && (setModel(null), setEffort(null)), setWs(w.id), setSheet(null))}>
                <span className="pick-icon">
                  <Icon name="folder" size={15} />
                </span>
                <span className="pick-body">
                  <b>{w.name}</b>
                  <small>
                    {w.machineName && `${w.machineName} · `}
                    {sessions(state.panes.filter((p) => p.workspaceId === w.id).length)}
                    {w.active && !(forMachine(all, w.machine).welcome?.server ?? activeIsServer()) ? " · open on the desktop" : ""}
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
              <button
                key={p.id}
                className="pick-row"
                data-on={p.id === chosenProvider?.id}
                onClick={() => {
                  haptic(5);
                  if (p.id !== chosenProvider?.id) {
                    setProvider(p.id);
                    setModel(null);
                    setEffort(null);
                  }
                  setSheet(null);
                }}
              >
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
      {sheet === "model" && (
        <ModelListSheet
          models={loadingModels ? null : models}
          selected={model}
          defaultLabel={defaultModelName}
          sub={chosenProvider?.name}
          providerIcon={chosenProvider?.id}
          onSelect={(nextModel) => {
            haptic(5);
            setModel(nextModel);
            const nextEntry = nextModel ? models.find((m) => m.id === nextModel) : null;
            const nextEfforts = nextEntry ? nextEntry.efforts : (catalog?.defaultEfforts ?? []);
            if (effort && !nextEfforts.includes(effort)) setEffort(null);
            setSheet(null);
          }}
          onClose={() => setSheet(null)}
        />
      )}
      {sheet === "effort" && (
        <EffortListSheet
          efforts={availableEfforts}
          selected={activeEffort}
          sub={effectiveModel?.name ?? chosenProvider?.name}
          onSelect={(nextEffort) => {
            haptic(5);
            setEffort(nextEffort === "default" ? null : nextEffort);
            setSheet(null);
          }}
          onClose={() => setSheet(null)}
        />
      )}
    </div>
  );
}
