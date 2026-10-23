import { GitBranch } from "lucide-react";
import logoUrl from "@/assets/logo.svg";
import { useCode, type Sheet } from "./store";
import { MODES } from "./protocol";

const STARS = Array.from({ length: 34 }, (_, i) => {
  // Deterministic scatter, so the field doesn't jump on re-render.
  const r = (n: number) => ((Math.sin(i * 12.9898 + n * 78.233) * 43758.5453) % 1 + 1) % 1;
  return { x: r(1) * 100, y: r(2) * 100, s: 1 + r(3) * 1.6, d: r(4) * 4, t: 2.5 + r(5) * 3 };
});

/** What a fresh session shows: the logo, who's answering and where, and the main keys. */
export function Welcome() {
  const { model, models, effort, mode, cwd, branch, setSheet, setDraft } = useCode();
  const label = models.find((m) => m.spec === model)?.label ?? model;
  const backend = model.split("/")[0];
  const m = MODES.find((x) => x.id === mode);
  const home = cwd?.replace(/^\/(Users|home)\/[^/]+/, "~");
  const tiles: { k: string; label: string; what: string; run: () => void }[] = [
    { k: "/", label: "commands", what: "every slash command", run: () => setDraft("/") },
    { k: "⇧⇥", label: "mode", what: "cycle permissions", run: () => setSheet("mode") },
    { k: "/model", label: "model", what: "switch backend", run: () => setSheet("model") },
    { k: "⌘V", label: "images", what: "paste or drop", run: () => {} },
    { k: "/resume", label: "sessions", what: "pick up a past one", run: () => openSessions() },
  ];
  const send = useCode((s) => s.send);
  const openSessions = () => {
    send({ type: "sessions" });
    setSheet("sessions" as Sheet);
  };
  return (
    <div className="cw-welcome">
      <div className="cw-stars" aria-hidden>
        {STARS.map((s, i) => (
          <i key={i} style={{ left: `${s.x}%`, top: `${s.y}%`, width: s.s, height: s.s, animationDelay: `${s.d}s`, animationDuration: `${s.t}s` }} />
        ))}
      </div>
      <div className="cw-welcome-inner">
        <div className="cw-logo-wrap">
          <img src={logoUrl} alt="winfer" className="cw-welcome-logo" />
        </div>
        <h1>
          winfer <span>code</span>
        </h1>
        <p className="cw-who">
          <span className={`cw-dot b-${backend}`} />
          {label || "Starting…"}
          {effort && effort !== "default" && <span className="dim"> · {effort}</span>}
          {m && <span className={`cw-mode-tag mode-${mode}`}>{m.label}</span>}
        </p>
        {home && (
          <p className="cw-where">
            {home}
            {branch && (
              <>
                <GitBranch size={12} /> {branch}
              </>
            )}
          </p>
        )}
        <div className="cw-tiles">
          {tiles.map((t, i) => (
            <button key={t.k} className="cw-tile" style={{ animationDelay: `${0.25 + i * 0.06}s` }} onClick={t.run}>
              <span>
                <b>{t.k}</b> {t.label}
              </span>
              <small>{t.what}</small>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
