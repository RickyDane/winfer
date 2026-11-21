import { useEffect, useMemo, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { DesktopClient } from "../client";
import { haptic } from "../hooks";
import type { ModelOption, Pane, PaneConfig } from "../protocol";
import { Icon, ProviderMark } from "./common";

/** The backends a model spec can run on (`claude/opus` → Claude Code), as the desktop names them. */
const BACKENDS: Record<string, string> = {
  chatgpt: "ChatGPT",
  claude: "Claude Code",
  codex: "Codex",
  agy: "Antigravity",
  cursor: "Cursor",
  opencode: "OpenCode",
  pi: "Pi",
  muse: "Muse Code",
  local: "Local model",
};
export const backendOf = (spec: string) => {
  const id = spec.split("/")[0];
  return BACKENDS[id] ? { id, name: BACKENDS[id] } : null;
};

export const EFFORT_LABELS: Record<string, string> = { default: "Default", none: "None", minimal: "Minimal", low: "Low", medium: "Medium", high: "High", xhigh: "Extra high", max: "Max", ultra: "Ultra" };
export const effortLabel = (level: string) => EFFORT_LABELS[level] ?? level.charAt(0).toUpperCase() + level.slice(1);
const EFFORT_HINTS: Record<string, string> = {
  default: "The model's own default",
  none: "No reasoning at all",
  minimal: "Barely any reasoning, fastest",
  low: "Quick answers, light reasoning",
  medium: "Balanced",
  high: "Thinks things through",
  xhigh: "Extra deliberate",
  max: "As much thinking as it takes",
  ultra: "The deepest it goes, slowest",
};

const BARS: Record<string, number> = { minimal: 1, low: 2, medium: 3, high: 4, xhigh: 5, max: 5, ultra: 5 };

/** Reasoning effort as rising bars, like the desktop's composer: more bars, more thinking. */
export function EffortIcon({ effort, size = 14 }: { effort: string; size?: number }) {
  const level = BARS[effort] ?? 0;
  const hot = effort === "max" || effort === "ultra";
  if (effort === "default") return <Icon name="sparkle" size={size - 1} className="effort-default" />;
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={`effort-bars${hot ? " hot" : ""}`} aria-hidden>
      {[0, 1, 2, 3, 4].map((i) => {
        const h = 3 + i * 2.6;
        return <rect key={i} x={1 + i * 3} y={15 - h} width={2} height={h} rx={1} className={i < level ? "on" : "off"} />;
      })}
    </svg>
  );
}

/** A bottom sheet in the desktop's popover look. */
export function Sheet({ title, sub, onClose, children, label }: { title: ReactNode; sub?: ReactNode; onClose: () => void; children: ReactNode; label: string }) {
  return createPortal(
    <div className="sheet-wrap" onClick={onClose}>
      <div className="sheet" role="dialog" aria-label={label} onClick={(e) => e.stopPropagation()}>
        <span className="sheet-grip" aria-hidden />
        <header>
          <div className="sheet-title">
            <b>{title}</b>
            {sub && <span>{sub}</span>}
          </div>
          <button className="icon-btn" aria-label="Close" onClick={onClose}>
            <Icon name="x" size={17} />
          </button>
        </header>
        {children}
      </div>
    </div>,
    document.body,
  );
}

function useSet(client: DesktopClient, pane: Pane, onDone?: () => void) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const set = async (change: { model?: string; effort?: string }) => {
    if (busy) return;
    setBusy(true);
    setErr(null);
    haptic(8);
    try {
      await client.request({ t: "pane.set", paneId: pane.id, ...change });
      onDone?.();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return { busy, err, set };
}

/** The reasoning levels the current model takes, with what each means. */
export function EffortPicker({ client, pane, config, online, onClose }: { client: DesktopClient; pane: Pane; config: PaneConfig; online: boolean; onClose: () => void }) {
  const { busy, err, set } = useSet(client, pane, onClose);
  return (
    <Sheet label="Reasoning effort" title="Reasoning" sub={config.label} onClose={onClose}>
      <div className="pick-list">
        {config.efforts.map((l) => (
          <button key={l} className="pick-row" data-on={l === config.effort} disabled={!online || busy} onClick={() => void set({ effort: l })}>
            <span className="pick-icon">
              <EffortIcon effort={l} size={15} />
            </span>
            <span className="pick-body">
              <b>{effortLabel(l)}</b>
              <small>{EFFORT_HINTS[l] ?? ""}</small>
            </span>
            {l === config.effort && <Icon name="check" size={16} className="pick-check" />}
          </button>
        ))}
      </div>
      {err && <div className="inline-error">{err}</div>}
    </Sheet>
  );
}

/** The models a winfer code chat can switch to, by backend, with a search once the list is long. */
export function ModelPicker({ client, pane, config, online, onClose }: { client: DesktopClient; pane: Pane; config: PaneConfig; online: boolean; onClose: () => void }) {
  const [models, setModels] = useState<ModelOption[] | null>(null);
  const [query, setQuery] = useState("");
  const [backend, setBackend] = useState<string | null>(null);
  const { busy, err, set } = useSet(client, pane, onClose);

  useEffect(() => {
    let live = true;
    client
      .request({ t: "config.get", paneId: pane.id })
      .then((r) => live && setModels(r.t === "config" ? r.models : []))
      .catch(() => live && setModels([]));
    return () => {
      live = false;
    };
  }, [client, pane.id]);

  const backends = useMemo(() => [...new Set((models ?? []).map((m) => m.id.split("/")[0]))], [models]);
  const q = query.trim().toLowerCase();
  const shown = (models ?? []).filter((m) => (!backend || m.id.split("/")[0] === backend) && (!q || `${m.name} ${m.id} ${m.detail ?? ""}`.toLowerCase().includes(q)));

  return (
    <Sheet label="Model" title="Model" sub={config.label || "Pick a model"} onClose={onClose}>
      {(models?.length ?? 0) > 6 && (
        <label className="pick-search">
          <Icon name="search" size={15} />
          <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search models…" enterKeyHint="search" />
        </label>
      )}
      {backends.length > 1 && (
        <div className="pick-rail">
          <button className="rail-chip" data-on={!backend} onClick={() => setBackend(null)}>
            All
          </button>
          {backends.map((b) => (
            <button key={b} className="rail-chip" data-on={backend === b} onClick={() => setBackend(b)}>
              <ProviderMark provider={b} size={12} />
              {BACKENDS[b] ?? b}
            </button>
          ))}
        </div>
      )}
      <div className="pick-list scrolls">
        {models === null && (
          <div className="loading">
            <span className="spinner" /> Loading models…
          </div>
        )}
        {models !== null && shown.length === 0 && <div className="loading">{models.length ? "No model matches." : "No models to pick from."}</div>}
        {shown.map((m) => {
          const on = m.id === config.model;
          return (
            <button key={m.id} className="pick-row" data-on={on} disabled={!online || busy} onClick={() => void set({ model: m.id })}>
              <span className="pick-icon mark">
                <ProviderMark provider={m.id.split("/")[0]} size={15} />
              </span>
              <span className="pick-body">
                <b>{m.name}</b>
                {m.detail && <small>{m.detail}</small>}
              </span>
              {on && <Icon name="check" size={16} className="pick-check" />}
            </button>
          );
        })}
      </div>
      {err && <div className="inline-error">{err}</div>}
    </Sheet>
  );
}
