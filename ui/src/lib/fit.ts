/** Fewest characters of a cut label that are worth drawing before its ellipsis. */
const MIN_CHARS = 4;

/**
 * `text` as it fits into `room` px: whole, or cut and ended with "…", or empty when fewer than 4 of its characters fit
 * beside the ellipsis. `width` measures a string in px (a canvas context's `measureText`).
 */
export function fitLabel(text: string, room: number, width: (s: string) => number): string {
  if (width(text) <= room) return text;
  let lo = 0;
  let hi = text.length - 1;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (width(`${text.slice(0, mid)}…`) <= room) lo = mid;
    else hi = mid - 1;
  }
  const kept = text.slice(0, lo).trimEnd();
  return kept.length < MIN_CHARS ? '' : `${kept}…`;
}
