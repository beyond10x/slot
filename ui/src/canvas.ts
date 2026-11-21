import { axisStepMs, axisTicks, fmtClock } from './lib/format.ts';
import type { RecordedBucket } from './lib/history.ts';

/** Height of the time axis strip at the bottom of a chart, px. */
export const AXIS = 20;
export const FONT = '11px system-ui, sans-serif';
export const RECORDED_CPU = '#436789';
export const RECORDED_QUEUE = '#78639a';

/** A separate horizontal stroke for each recorded interval; missing intervals remain blank. */
export function drawRecorded(ctx: CanvasRenderingContext2D, buckets: readonly RecordedBucket[],
  field: 'host_cores' | 'waiting', X: (t: number) => number, Y: (v: number) => number): void {
  ctx.fillStyle = field === 'waiting' ? RECORDED_QUEUE : RECORDED_CPU;
  for (const b of buckets) {
    const x = X(b.t);
    ctx.fillRect(x, Math.round(Y(b[field])) - 1, X(b.t + b.n * 1000) - x, 2);
  }
}

/** Connect adjacent ticks only. A single isolated measurement still gets a mark. */
export function drawExact<T extends { t: number }>(ctx: CanvasRenderingContext2D, points: readonly T[],
  X: (t: number) => number, y: (p: T) => number, step = false): void {
  ctx.beginPath();
  points.forEach((p, i) => {
    const prev = points[i - 1];
    const x = X(p.t);
    if (prev && p.t - prev.t < 2000) {
      if (step) ctx.lineTo(x, y(prev));
      ctx.lineTo(x, y(p));
    } else {
      ctx.moveTo(x - 0.5, y(p));
      ctx.lineTo(x + 0.5, y(p));
    }
  });
  ctx.stroke();
}

/** Sizes the backing store to CSS width x `h` at device pixel ratio; returns the CSS width. */
export function sizeCanvas(c: HTMLCanvasElement, ctx: CanvasRenderingContext2D, h: number): number {
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth;
  if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
    c.style.height = h + 'px';
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return w;
}

/** Vertical grid lines and clock labels for [t0, t1]. */
export function drawAxis(ctx: CanvasRenderingContext2D, w: number, h: number, t0: number, t1: number): void {
  const step = axisStepMs((t1 - t0) / 1000);
  ctx.font = FONT;
  ctx.textBaseline = 'top';
  for (const t of axisTicks(t0, t1, step)) {
    const x = ((t - t0) / (t1 - t0)) * w;
    ctx.fillStyle = '#eee';
    ctx.fillRect(x, 0, 1, h - AXIS + 4);
    ctx.fillStyle = '#666';
    ctx.fillText(fmtClock(t, step < 60_000), x + 2, h - AXIS + 6);
  }
}

/** Runs `draw` on every animation frame until the returned stop function is called. */
export function animate(draw: () => void): () => void {
  let id = 0;
  const loop = () => {
    draw();
    id = requestAnimationFrame(loop);
  };
  id = requestAnimationFrame(loop);
  return () => cancelAnimationFrame(id);
}
