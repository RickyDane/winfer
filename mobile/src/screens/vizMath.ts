// The numbers behind the phone's charts: compact number text and axis ticks.
// (Port of the desktop's `code/viz.rs`; no UI here.)

/** "1.2k", "3.4M", "0.25", with the unit after it ("%" hugs the number). */
export function fmt(n: number, unit?: string | null): string {
  const a = Math.abs(n);
  let text: string;
  if (a >= 1e9) text = `${trim(n / 1e9)}B`;
  else if (a >= 1e6) text = `${trim(n / 1e6)}M`;
  else if (a >= 1e4) text = `${trim(n / 1e3)}k`;
  else text = trim(n);
  if (!unit) return text;
  return unit === "%" ? `${text}%` : `${text} ${unit}`;
}

function trim(n: number): string {
  const a = Math.abs(n);
  const digits = a >= 100 ? 0 : a >= 10 ? 1 : 2;
  return Number(n.toFixed(digits)).toString();
}

/** Round axis ticks from 0 (or below, for negatives) over the data's range: `[min, ..., max]`. */
export function ticks(lo: number, hi: number, want = 4): number[] {
  const min = Math.min(lo, 0);
  const max = Math.max(hi, 0);
  if (max === min) return [0, 1];
  const raw = (max - min) / want;
  const pow = 10 ** Math.floor(Math.log10(raw));
  const f = raw / pow;
  const step = (f <= 1.5 ? 1 : f <= 3 ? 2 : f <= 7 ? 5 : 10) * pow;
  const out: number[] = [];
  for (let v = Math.floor(min / step) * step; v < max + step - 1e-9; v += step) out.push(Number(v.toPrecision(12)));
  return out;
}

/** One color per series or slice, in a fixed order that reads on dark and light themes. */
export const PALETTE = ["#7c8cff", "#34d399", "#fbbf24", "#f472b6", "#38bdf8", "#fb923c", "#a78bfa", "#2dd4bf", "#f87171", "#a3e635", "#e879f9", "#94a3b8"];
export const color = (i: number) => PALETTE[i % PALETTE.length] as string;

/** Pie slices as `[start, end]` angles in turns (0..1), skipping nothing (zero slices are empty). */
export function arcs(values: number[]): [number, number][] {
  const total = values.reduce((a, b) => a + b, 0) || 1;
  let at = 0;
  return values.map((v) => {
    const start = at;
    at += v / total;
    return [start, at];
  });
}
