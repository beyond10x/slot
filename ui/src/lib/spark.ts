export interface Point {
  t: number;
  v: number;
}

/** Per-job recent samples, kept for `spanMs` behind the newest sample of each job. */
export class SparkBuffer {
  readonly spanMs: number;
  private readonly byId = new Map<number, Point[]>();

  constructor(spanMs = 60_000) {
    this.spanMs = spanMs;
  }

  push(id: number, t: number, v: number): void {
    let pts = this.byId.get(id);
    if (!pts) {
      pts = [];
      this.byId.set(id, pts);
    }
    if (pts.length && t <= pts[pts.length - 1].t) return;
    pts.push({ t, v });
    let drop = 0;
    while (drop < pts.length && pts[drop].t < t - this.spanMs) drop++;
    if (drop) pts.splice(0, drop);
  }

  /** Forgets every job not in `ids` (they stopped running). */
  retain(ids: ReadonlySet<number>): void {
    for (const id of [...this.byId.keys()]) if (!ids.has(id)) this.byId.delete(id);
  }

  get(id: number): readonly Point[] {
    return this.byId.get(id) ?? [];
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
