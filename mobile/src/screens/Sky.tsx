import { useEffect, useState, type CSSProperties } from "react";

// The night sky behind a fresh session, as winfer code paints it (`Welcome.tsx`, `render_space_sky`):
// a faint nebula and Milky Way band, three depths of twinkling stars and the odd shooting star.

// Deterministic scatter, so the field doesn't jump on re-render.
const rand = (i: number, n: number) => (((Math.sin(i * 12.9898 + n * 78.233) * 43758.5453) % 1) + 1) % 1;
const TINTS = ["", "", "", "cool", "cool", "warm"];
type Star = { x: number; y: number; s: number; d: number; t: number; tint: string };
const field = (count: number, seed: number, min: number, spread: number): Star[] =>
  Array.from({ length: count }, (_, k) => {
    const i = k + seed;
    return { x: rand(i, 1) * 100, y: rand(i, 2) * 100, s: min + rand(i, 3) * spread, d: rand(i, 4) * 6, t: 2.5 + rand(i, 5) * 4, tint: TINTS[Math.floor(rand(i, 6) * TINTS.length)] };
  });
/** Fewer than the desktop's: a phone is a fraction of a pane. */
const LAYERS = [
  { cls: "far", stars: field(70, 0, 0.6, 0.8) },
  { cls: "mid", stars: field(28, 300, 1.1, 1.2) },
  { cls: "near", stars: field(5, 600, 2, 1.2) },
];

type Shot = { id: number; x: number; y: number; a: number; len: number; dist: number; dur: number };

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
      const shot: Shot = { id: ++id, x: leftward ? 35 + Math.random() * 60 : 5 + Math.random() * 60, y: Math.random() * 55, a: leftward ? 180 - tilt : tilt, len: 60 + Math.random() * 90, dist: 140 + Math.random() * 220, dur };
      setShots((s) => [...s, shot]);
      const t = setTimeout(() => {
        timers.delete(t);
        setShots((s) => s.filter((x) => x.id !== shot.id));
      }, dur * 1000 + 200);
      timers.add(t);
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

export function Sky() {
  const shots = useShootingStars();
  return (
    <div className="sky" aria-hidden>
      <div className="sky-nebula" />
      <div className="sky-milkyway" />
      {LAYERS.map((l) => (
        <div key={l.cls} className={`sky-layer ${l.cls}`}>
          {l.stars.map((s, i) => (
            <i key={i} className={s.tint} style={{ left: `${s.x}%`, top: `${s.y}%`, width: s.s, height: s.s, animationDelay: `${s.d}s`, animationDuration: `${s.t}s` }} />
          ))}
        </div>
      ))}
      {shots.map((s) => (
        <span key={s.id} className="sky-shoot" style={{ left: `${s.x}%`, top: `${s.y}%`, width: s.len, transform: `rotate(${s.a}deg)`, "--dist": `${s.dist}px`, "--dur": `${s.dur}s` } as CSSProperties} />
      ))}
    </div>
  );
}
