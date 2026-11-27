/**
 * Procedural nebula face for the send / `+` button, matching `native/crates/winfer-app/src/ui/nebula.rs`
 * pixel-for-pixel (domain-warped 5-octave FBM clouds in three dye colours, stars and a dark rim).
 */

export type NebulaKind = "ready" | "idle" | "stop";

const SIZE = 96;

type Rgb = [number, number, number];

function hash(x: number, y: number, seed: number): number {
  let h = (Math.imul(x | 0, 0x27d4eb2d) ^ Math.imul(y | 0, 0x165667b1) ^ Math.imul(seed | 0, 0x9e3779b9)) >>> 0;
  h = (h ^ (h >>> 15)) >>> 0;
  h = Math.imul(h, 0x85ebca6b) >>> 0;
  h = (h ^ (h >>> 13)) >>> 0;
  h = Math.imul(h, 0xc2b2ae35) >>> 0;
  h = (h ^ (h >>> 16)) >>> 0;
  return (h & 0xffff) / 65535;
}

function valueNoise(x: number, y: number, seed: number): number {
  const xi = Math.floor(x);
  const yi = Math.floor(y);
  const fx = x - xi;
  const fy = y - yi;
  const sx = fx * fx * (3 - 2 * fx);
  const sy = fy * fy * (3 - 2 * fy);
  const a = hash(xi, yi, seed);
  const b = hash(xi + 1, yi, seed);
  const c = hash(xi, yi + 1, seed);
  const d = hash(xi + 1, yi + 1, seed);
  return a + (b - a) * sx + (c + (d - c) * sx - (a + (b - a) * sx)) * sy;
}

function fbm(x: number, y: number, seed: number): number {
  let sum = 0;
  let amp = 0.5;
  let freq = 1;
  for (let o = 0; o < 5; o++) {
    sum += amp * valueNoise(x * freq, y * freq, seed + o);
    amp *= 0.5;
    freq *= 2.03;
  }
  return sum;
}

function smooth(lo: number, hi: number, v: number): number {
  const t = Math.max(0, Math.min(1, (v - lo) / (hi - lo)));
  return t * t * (3 - 2 * t);
}

function clamp01(v: number): number {
  return v < 0 ? 0 : v > 1 ? 1 : v;
}

function hexc(s: string): Rgb {
  const v = Number.parseInt(s.replace(/^#/, ""), 16) || 0;
  return [((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255];
}

function mix(a: Rgb, b: Rgb, t: number): Rgb {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
}

interface Palette {
  bg: Rgb;
  dye1: Rgb;
  dye2: Rgb;
  dye3: Rgb;
  strength: number;
}

const PALETTES: Record<`${NebulaKind}:${"dark" | "light"}`, Palette> = {
  "ready:dark": { bg: hexc("#07030f"), dye1: hexc("#5b21b6"), dye2: hexc("#d946ef"), dye3: hexc("#38bdf8"), strength: 1.0 },
  "stop:dark": { bg: hexc("#0f0403"), dye1: hexc("#9a3412"), dye2: hexc("#e11d48"), dye3: hexc("#fbbf24"), strength: 1.0 },
  "idle:dark": { bg: hexc("#07050f"), dye1: hexc("#3b2a7a"), dye2: hexc("#5b2a86"), dye3: hexc("#1d4e89"), strength: 0.55 },
  "ready:light": { bg: hexc("#6366f1"), dye1: hexc("#8b5cf6"), dye2: hexc("#e879f9"), dye3: hexc("#38bdf8"), strength: 1.0 },
  "stop:light": { bg: hexc("#e11d48"), dye1: hexc("#f97316"), dye2: hexc("#fb7185"), dye3: hexc("#fde047"), strength: 1.0 },
  "idle:light": { bg: hexc("#eef0f8"), dye1: hexc("#b4c0f8"), dye2: hexc("#dfc4fb"), dye3: hexc("#bae6fd"), strength: 0.65 },
};

/** Paints the square RGBA pixel buffer of the desktop nebula at `phase` radians. */
export function paintNebulaRgba(kind: NebulaKind, light: boolean, phase = 0, size = SIZE): Uint8ClampedArray {
  const ox = Math.cos(phase) * 0.55;
  const oy = Math.sin(phase) * 0.55;
  const { bg, dye1, dye2, dye3, strength } = PALETTES[`${kind}:${light ? "light" : "dark"}`];
  const starThresh = 0.9965;
  const out = new Uint8ClampedArray(size * size * 4);
  let i = 0;

  for (let py = 0; py < size; py++) {
    const v = py / size;
    const dy = v - 0.5;
    for (let px = 0; px < size; px++) {
      const u = px / size;
      const wx = fbm(u * 2.6 + 3.1 + ox, v * 2.6 + oy, 11);
      const wy = fbm(u * 2.6 - oy, v * 2.6 + 7.7 + ox, 12);
      const x = u * 2.8 + wx * 1.7;
      const y = v * 2.8 + wy * 1.7;
      const d1 = fbm(x, y, 1);
      const d2 = fbm(x + 5.2, y + 1.3, 2);
      const d3 = fbm(x - 3.7, y + 8.1, 3);
      const a1 = smooth(0.28, 0.78, d1);
      const a2 = smooth(0.4, 0.85, d2) * 0.9;
      const s3 = smooth(0.5, 0.9, d3);
      const a3 = s3 * s3 * 0.8;
      const core = smooth(0.55, 1.0, (d1 + d2) * 0.5);
      const dx = Math.abs(u - 0.5);
      const r = Math.hypot(dx, dy) * 2.0;
      const star = hash(px, py, 99);
      const twinkle = star > starThresh ? 0.6 + 0.4 * hash(px, py, 7) : 0;

      let r0: number;
      let g0: number;
      let b0: number;
      if (light) {
        let c = mix(bg, dye1, clamp01(a1 * 0.75 * strength));
        c = mix(c, dye2, clamp01(a2 * 0.7 * strength));
        c = mix(c, dye3, clamp01(a3 * 0.62 * strength));
        const lift = core * (kind === "idle" ? 0.05 : 0.12);
        const rimDepth = kind === "idle" ? 0.09 : 0.2;
        const rim = 1 - smooth(0.55, 1.05, r) * rimDepth;
        const starK = kind === "idle" ? 0.18 : 0.55;
        r0 = clamp01((c[0] + lift) * rim + twinkle * starK);
        g0 = clamp01((c[1] + lift) * rim + twinkle * starK);
        b0 = clamp01((c[2] + lift) * rim + twinkle * starK);
      } else {
        const lift = core * 0.12 * strength;
        const rim = 1 - smooth(0.55, 1.05, r) * 0.82;
        const starK = kind === "idle" ? 0.5 : 1.0;
        const c0 = bg[0] + (dye1[0] * a1 + dye2[0] * a2 + dye3[0] * a3) * strength * 0.75 + lift;
        const c1 = bg[1] + (dye1[1] * a1 + dye2[1] * a2 + dye3[1] * a3) * strength * 0.75 + lift;
        const c2 = bg[2] + (dye1[2] * a1 + dye2[2] * a2 + dye3[2] * a3) * strength * 0.75 + lift;
        r0 = Math.pow(clamp01(c0 * rim + twinkle * starK), 0.92);
        g0 = Math.pow(clamp01(c1 * rim + twinkle * starK), 0.92);
        b0 = Math.pow(clamp01(c2 * rim + twinkle * starK), 0.92);
      }

      out[i++] = (clamp01(r0) * 255) | 0;
      out[i++] = (clamp01(g0) * 255) | 0;
      out[i++] = (clamp01(b0) * 255) | 0;
      out[i++] = 255;
    }
  }
  return out;
}

const urlCache = new Map<string, string>();

/** Returns a cached `data:image/png` URL for the given nebula state and theme mode. */
export function nebulaDataUrl(kind: NebulaKind, light: boolean): string | null {
  if (typeof document === "undefined") return null;
  const key = `${kind}:${light ? "light" : "dark"}`;
  const cached = urlCache.get(key);
  if (cached) return cached;
  const canvas = document.createElement("canvas");
  canvas.width = SIZE;
  canvas.height = SIZE;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const rgba = paintNebulaRgba(kind, light, 0, SIZE);
  const img = ctx.createImageData(SIZE, SIZE);
  img.data.set(rgba);
  ctx.putImageData(img, 0, 0);
  const url = canvas.toDataURL("image/png");
  urlCache.set(key, url);
  return url;
}

/** Paints and binds the `--nebula-*` CSS variables on `:root` for the active light/dark mode. */
export function applyNebulaVars(light: boolean): void {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  const entries: [string, NebulaKind][] = [
    ["--nebula-send", "ready"],
    ["--nebula-idle", "idle"],
    ["--nebula-stop", "stop"],
  ];
  for (const [prop, kind] of entries) {
    const url = nebulaDataUrl(kind, light);
    if (url) root.style.setProperty(prop, `url("${url}")`);
  }
}
