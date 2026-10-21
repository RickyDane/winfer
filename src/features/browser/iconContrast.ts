/** Picks out page icons that would vanish against the tab bar, and how to make them visible. */

type Rgb = [number, number, number];

/** WCAG relative luminance of an sRGB colour (0–255 channels). */
export function luminance([r, g, b]: Rgb) {
  const lin = (c: number) => {
    const v = c / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

export function contrast(a: number, b: number) {
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

/** An icon pixel at least this far from the background counts as visible. */
const VISIBLE_CONTRAST = 1.8;
/** Below this share of visible (opaque) pixels, the icon is adjusted. */
const MIN_VISIBLE_SHARE = 0.25;

/**
 * Whether an icon's pixels (RGBA, as from `getImageData`) mostly blend into a background of
 * luminance `bg`. Transparent pixels don't count; an icon with its own opaque backdrop passes.
 */
export function blendsIn(pixels: Uint8ClampedArray, bg: number) {
  let total = 0;
  let visible = 0;
  for (let i = 0; i < pixels.length; i += 4) {
    const alpha = pixels[i + 3] / 255;
    if (alpha < 0.3) continue;
    total += alpha;
    if (contrast(luminance([pixels[i], pixels[i + 1], pixels[i + 2]]), bg) >= VISIBLE_CONTRAST) visible += alpha;
  }
  return total > 0 && visible / total < MIN_VISIBLE_SHARE;
}

/** Flips lightness but keeps hues, so a dark logo turns light (and the other way round). */
export const INVERT_FILTER = "invert(1) hue-rotate(180deg)";

/** The colour actually painted behind an element: its first ancestor with an opaque-ish background. */
export function backgroundLuminance(el: Element | null): number | null {
  for (let node = el; node; node = node.parentElement) {
    const rgb = parseColor(getComputedStyle(node).backgroundColor);
    if (rgb) return luminance(rgb);
  }
  return null;
}

function parseColor(css: string): Rgb | null {
  const m = css.match(/rgba?\(([^)]+)\)/);
  if (!m) return null;
  const [r, g, b, a = 1] = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
  return a >= 0.5 ? [r, g, b] : null;
}

/** Reads an icon's pixels at a small size; null when they can't be read (a cross-origin image). */
export function iconPixels(img: HTMLImageElement, size = 32): Uint8ClampedArray | null {
  try {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = size;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return null;
    ctx.drawImage(img, 0, 0, size, size);
    return ctx.getImageData(0, 0, size, size).data;
  } catch {
    return null;
  }
}
