import { mergeByT } from './history.ts';

export interface Point {
  t: number;
  v: number;
}

/** Recent samples per key (job id, session), kept for `spanMs` behind the newest sample of each key. */
export class SparkBuffer<K = number> {
  readonly spanMs: number;
  private readonly byId = new Map<K, Point[]>();

  constructor(spanMs = 60_000) {
    this.spanMs = spanMs;
  }

  push(id: K, t: number, v: number): void {
    let pts = this.byId.get(id);
    if (!pts) {
      pts = [];
      this.byId.set(id, pts);
    }
    if (pts.length && t <= pts[pts.length - 1].t) return;
    pts.push({ t, v });
    this.prune(pts);
  }

  /** Merges older points (sorted by t) under the ones already held; a point at an existing t is not added twice. */
  seed(id: K, older: readonly Point[]): void {
    const held = this.byId.get(id) ?? [];
    const merged = mergeByT(older, held);
    this.prune(merged);
    if (merged.length) this.byId.set(id, merged);
  }

  /** Forgets every key not in `ids` (job stopped running, session gone). */
  retain(ids: ReadonlySet<K>): void {
    for (const id of [...this.byId.keys()]) if (!ids.has(id)) this.byId.delete(id);
  }

  get(id: K): readonly Point[] {
    return this.byId.get(id) ?? [];
  }

  private prune(pts: Point[]): void {
    if (!pts.length) return;
    const newest = pts[pts.length - 1].t;
    let drop = 0;
    while (drop < pts.length && pts[drop].t < newest - this.spanMs) drop++;
    if (drop) pts.splice(0, drop);
  }

  get size(): number {
    return this.byId.size;
  }
}

/**
 * SVG path of a step line over [t1 - span, t1] in a w x h box, values scaled
 * to `vmax` (0 at the bottom). Empty string when there are no points.
 */
export function sparkPath(pts: readonly Point[], t1: number, span: number, w: number, h: number, vmax: number): string {
  if (!pts.length) return '';
  const t0 = t1 - span;
  const x = (t: number) => Math.round(Math.min(w, Math.max(0, ((t - t0) / span) * w)) * 10) / 10;
  const y = (v: number) => Math.round((h - (Math.min(v, vmax) / vmax) * h) * 10) / 10;
  let d = `M${x(pts[0].t)},${y(pts[0].v)}`;
  for (let k = 1; k < pts.length; k++) d += `H${x(pts[k].t)}V${y(pts[k].v)}`;
  return d + `H${x(t1)}`;
}
