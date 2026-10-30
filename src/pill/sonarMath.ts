// Pure helpers for the pill's listening waves: a fixed loop of staggered waves rising out of the bars.
// No DOM and no allocation after setup, so the pill's rAF loop can call these every frame.

export const WAVE_MS = 1800;

export const easeOut = (p: number) => 1 - (1 - p) * (1 - p);

export type WaveLook = { rise: number; spread: number; opacity: number; stroke: number };
export const createLook = (): WaveLook => ({ rise: 0, spread: 0, opacity: 0, stroke: 0 });

/**
 * Wave `i` of `n`, `elapsed` ms after listening began, written into `out`. False until its stagger has passed.
 * rise 0 → 1 (bar line → top of travel); spread 0.4 → 1 (share of the bar width, never wider);
 * opacity fades in over the first 10 % from 0 to ~`peak`, then falls to 0; stroke thins 2 → 1 px.
 */
export function waveLook(elapsed: number, i: number, n: number, peak: number, out: WaveLook): boolean {
  const local = elapsed - (i * WAVE_MS) / n;
  if (local < 0) return false;
  const p = (local % WAVE_MS) / WAVE_MS;
  const e = easeOut(p);
  out.rise = e;
  out.spread = 0.4 + 0.6 * e;
  out.opacity = peak * Math.min(1, p / 0.1) * (1 - p);
  out.stroke = 2 - e;
  return true;
}
