import { axisStepMs, axisTicks, fmtClock } from './lib/format.ts';
import type { RecordedBucket } from './lib/history.ts';

/** Height of the time axis strip at the bottom of a chart, px. */
export const AXIS = 20;
export const FONT = '12px system-ui, sans-serif';
/** Colour of the labels drawn in a chart: the page's quiet text colour. */
export const INK = '#5c5c5c';
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

/**
 * One level stroke per point over the `span` ms that end at its `t`; a point that follows within one and a half spans
 * is joined to it by a step. A point without a value (`y` not finite) and a time without a point stay blank.
 */
export function drawSpans<T extends { t: number }>(ctx: CanvasRenderingContext2D, points: readonly T[],
  X: (t: number) => number, y: (p: T) => number, span: number): void {
  ctx.beginPath();
  let prev: T | undefined;
  for (const p of points) {
    const v = y(p);
    if (!Number.isFinite(v)) {
      prev = undefined;
      continue;
    }
    if (prev && p.t - prev.t <= span * 1.5) ctx.lineTo(X(prev.t), v);
    else ctx.moveTo(X(p.t - span), v);
    ctx.lineTo(X(p.t), v);
    prev = p;
  }
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

/**
 * Vertical grid lines and clock labels for [t0, t1]. A label stands right of its line; the last one moves left
 * to end inside the chart, and is left out where that would touch the label before it, so no label is cut.
 */
export function drawAxis(ctx: CanvasRenderingContext2D, w: number, h: number, t0: number, t1: number): void {
  const step = axisStepMs((t1 - t0) / 1000);
  ctx.font = FONT;
  ctx.textBaseline = 'top';
  let lastEnd = -Infinity;
  for (const t of axisTicks(t0, t1, step)) {
    const x = ((t - t0) / (t1 - t0)) * w;
    ctx.fillStyle = '#ececec';
    ctx.fillRect(x, 0, 1, h - AXIS + 4);
    const label = fmtClock(t, step < 60_000);
    const tw = ctx.measureText(label).width;
    const lx = Math.min(x + 3, w - tw - 2);
    if (lx < lastEnd + 8) continue;
    ctx.fillStyle = INK;
    ctx.fillText(label, lx, h - AXIS + 5);
    lastEnd = lx + tw;
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
