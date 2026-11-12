/** Whether a scroll box is at its end, within `slack` px. */
export function atBottom(scrollTop: number, clientHeight: number, scrollHeight: number, slack = 20): boolean {
  return scrollTop + clientHeight >= scrollHeight - slack;
}

/**
 * Page scroll, px, that brings a box (`top`..`bottom`, viewport coordinates) into the band between
 * `visTop` and `visBottom` with `margin` to spare; 0 when it is in the band. A box taller than the
 * band is aligned at its top.
 */
export function revealDelta(top: number, bottom: number, visTop: number, visBottom: number, margin = 8): number {
  if (top - margin < visTop || bottom - top > visBottom - visTop - 2 * margin) return top - margin - visTop;
  if (bottom + margin > visBottom) return bottom + margin - visBottom;
  return 0;
}
