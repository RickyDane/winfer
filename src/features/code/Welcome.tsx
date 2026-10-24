import { useEffect, useState } from "react";
import { GitBranch } from "lucide-react";
import logoUrl from "@/assets/logo.svg";
import { useCode, type Sheet } from "./store";
import { MODES } from "./protocol";
import { useCodeWeb } from "./styles";
import { backendOf, modelName } from "./names";
import { ProviderIcon } from "@/features/providers/ProviderIcon";

// Deterministic scatter, so the field doesn't jump on re-render.
const rand = (i: number, n: number) => ((Math.sin(i * 12.9898 + n * 78.233) * 43758.5453) % 1 + 1) % 1;
const TINTS = ["", "", "", "cool", "cool", "warm"];
type Star = { x: number; y: number; s: number; d: number; t: number; tint: string };
const field = (count: number, seed: number, min: number, spread: number): Star[] =>
  Array.from({ length: count }, (_, k) => {
    const i = k + seed;
    return {
      x: rand(i, 1) * 100,
      y: rand(i, 2) * 100,
      s: min + rand(i, 3) * spread,
      d: rand(i, 4) * 6,
      t: 2.5 + rand(i, 5) * 4,
      tint: TINTS[Math.floor(rand(i, 6) * TINTS.length)],
    };
  });
/** Three depths: faint dust, the usual stars, and a few bright ones with a glint. */
const LAYERS: { cls: string; stars: Star[] }[] = [
  { cls: "far", stars: field(110, 0, 0.6, 0.8) },
  { cls: "mid", stars: field(42, 300, 1.1, 1.2) },
  { cls: "near", stars: field(7, 600, 2, 1.2) },
];

type Shot = { id: number; x: number; y: number; a: number; len: number; dist: number; dur: number };

/** Now and then a shooting star crosses the sky, at a random spot and angle. */
function useShootingStars(): Shot[] {
  const [shots, setShots] = useState<Shot[]>([]);
  useEffect(() => {
    if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    let id = 0;
    let timer: ReturnType<typeof setTimeout>;
    const timers = new Set<ReturnType<typeof setTimeout>>();
    const fire = () => {
      const leftward = Math.random() < 0.5;
      const tilt = 12 + Math.random() * 30;
      const dur = 0.7 + Math.random() * 0.9;
      const shot: Shot = {
        id: ++id,
        x: leftward ? 35 + Math.random() * 60 : 5 + Math.random() * 60,
        y: Math.random() * 55,
        a: leftward ? 180 - tilt : tilt,
        len: 70 + Math.random() * 110,
        dist: 180 + Math.random() * 320,
        dur,
      };
      setShots((s) => [...s, shot]);
      const t = setTimeout(() => {
        timers.delete(t);
        setShots((s) => s.filter((x) => x.id !== shot.id));
      }, dur * 1000 + 200);
      timers.add(t);
      // Sometimes a second one right after, like a small shower.
      timer = setTimeout(fire, Math.random() < 0.15 ? 350 + Math.random() * 500 : 3500 + Math.random() * 8000);
    };
    timer = setTimeout(fire, 1200 + Math.random() * 2000);
    return () => {
      clearTimeout(timer);
      timers.forEach(clearTimeout);
    };
  }, []);
  return shots;
}

/** What a fresh session shows: the logo, who's answering and where, and the main keys. */
export function Welcome() {
  const { model, actualModel, effort, mode, cwd, branch, setSheet, setDraft } = useCode();
  const resolved = useCodeWeb((s) => s.resolvedModels);
  const label = model ? modelName(model, resolved, actualModel) : "";
  const m = MODES.find((x) => x.id === mode);
  const home = cwd?.replace(/^\/(Users|home)\/[^/]+/, "~");
  const tiles: { k: string; label: string; what: string; run: () => void }[] = [
    { k: "/", label: "commands", what: "every slash command", run: () => setDraft("/") },
    { k: "⇧⇥", label: "mode", what: "cycle permissions", run: () => setSheet("mode") },
    { k: "/model", label: "model", what: "switch backend", run: () => setSheet("model") },
    { k: "/resume", label: "sessions", what: "pick up a past one", run: () => openSessions() },
  ];
  const send = useCode((s) => s.send);
  const shots = useShootingStars();
  const openSessions = () => {
    send({ type: "sessions" });
    setSheet("sessions" as Sheet);
  };
  return (
    <div className="cw-welcome">
      <div className="cw-stars" aria-hidden>
        <div className="cw-nebula" />
        <div className="cw-milkyway" />
        {LAYERS.map((l) => (
          <div key={l.cls} className={`cw-star-layer ${l.cls}`}>
            {l.stars.map((s, i) => (
              <i
                key={i}
                className={s.tint}
                style={{ left: `${s.x}%`, top: `${s.y}%`, width: s.s, height: s.s, animationDelay: `${s.d}s`, animationDuration: `${s.t}s` }}
              />
            ))}
          </div>
        ))}
        {shots.map((s) => (
          <span
            key={s.id}
            className="cw-shoot"
            style={
              {
                left: `${s.x}%`,
                top: `${s.y}%`,
                width: s.len,
                transform: `rotate(${s.a}deg)`,
                "--dist": `${s.dist}px`,
                "--dur": `${s.dur}s`,
              } as React.CSSProperties
            }
          />
        ))}
      </div>
      <div className="cw-welcome-inner">
        <div className="cw-logo-wrap">
          <span className="cw-orbit" aria-hidden />
          <img src={logoUrl} alt="winfer" className="cw-welcome-logo" />
        </div>
        <h1>
          winfer <span>code</span>
        </h1>
        <p className="cw-who">
          <ProviderIcon provider={backendOf(model)?.logo ?? "winfer"} size={14} />
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
