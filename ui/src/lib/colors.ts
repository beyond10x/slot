/** Fixed toolchain colours (Okabe-Ito vermillion and blue, neutral grey). */
export const BASE_COLORS: Readonly<Record<string, string>> = {
  rust: '#D55E00',
  go: '#0072B2',
  other: '#8c8c8c',
};

/** Queue length series (Okabe-Ito reddish purple, distinct from the toolchain colours). */
export const QUEUE_COLOR = '#CC79A7';

/** Stable hue 0..359 for a name. */
export function hashHue(name: string): number {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}

const cache = new Map<string, string>(Object.entries(BASE_COLORS));

/** Colour of a toolchain: fixed for known ones, a hashed hsl otherwise. */
export function tcColor(name: string): string {
  let c = cache.get(name);
  if (c === undefined) {
    c = `hsl(${hashHue(name)},60%,42%)`;
    cache.set(name, c);
  }
  return c;
}

/** Stacking order: the fixed toolchains first, then others in first-seen order. */
export function stackOrder(names: Iterable<string>): string[] {
  const out = Object.keys(BASE_COLORS);
  for (const n of names) if (!out.includes(n)) out.push(n);
  return out;
}
