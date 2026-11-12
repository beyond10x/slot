/** Largest headroom the daemon accepts, percent (`limits::MAX_HEADROOM`). */
export const MAX_HEADROOM = 50;

/** Slider step, percent. */
export const HEADROOM_STEP = 5;

/** Tokens that circulate with `headroom` percent kept free; mirrors `limits::effective_tokens`. */
export function effectiveTokens(tokens: number, headroom: number): number {
  const h = Math.min(Math.max(0, Math.round(headroom)), MAX_HEADROOM);
  return Math.max(1, Math.floor((tokens * (100 - h) + 50) / 100));
}

/** Cores builds may use together under the CPU quota for `headroom`. */
export function buildCores(cores: number, headroom: number): number {
  const h = Math.min(Math.max(0, Math.round(headroom)), MAX_HEADROOM);
  return (cores * (100 - h)) / 100;
}

/** The slider's caption, for example "builds may use 14 of 20 cores, 14 of 20 slots". */
export function headroomCaption(cores: number, tokens: number, headroom: number): string {
  const c = buildCores(cores, headroom);
  const shown = Number.isInteger(c) ? String(c) : c.toFixed(1);
  return `builds may use ${shown} of ${cores} cores, ${effectiveTokens(tokens, headroom)} of ${tokens} slots`;
}

/**
 * Error text for an answer from `PUT api/limits` that is not JSON, or null
 * when it is. A daemon older than 0.3.0 has no such path and answers with
 * this page (200, text/html).
 */
export function notJsonError(contentType: string | null): string | null {
  if (contentType?.trim().toLowerCase().startsWith('application/json')) return null;
  return 'the daemon is older than this page and has no headroom; upgrade it (systemctl --user reload slot)';
}
