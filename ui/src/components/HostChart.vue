<script setup lang="ts">
import { onMounted, onUnmounted, useTemplateRef } from 'vue';
import { AXIS, FONT, RECORDED_CPU, animate, drawAxis, drawExact, drawRecorded, sizeCanvas } from '../canvas.ts';
import { familyColor, stack } from '../lib/host.ts';
import { chartWindow, now, store } from '../store.ts';

const H = 160;
const TOP_PAD = 16;

const cv = useTemplateRef<HTMLCanvasElement>('cv');

/** Light diagonal stripes laid over the unmanaged (not on slot) bands. */
function hatch(ctx: CanvasRenderingContext2D): CanvasPattern | null {
  const tile = document.createElement('canvas');
  tile.width = tile.height = 6;
  const t = tile.getContext('2d');
  if (!t) return null;
  t.strokeStyle = 'rgba(255,255,255,0.85)';
  t.lineWidth = 1.5;
  t.beginPath();
  t.moveTo(0, 6);
  t.lineTo(6, 0);
  t.stroke();
  return ctx.createPattern(tile, 'repeat');
}

function draw(c: HTMLCanvasElement, ctx: CanvasRenderingContext2D, stripes: CanvasPattern | null): void {
  const win = store.windowS * 1000;
  const t1 = now();
  const t0 = t1 - win;
  const w = sizeCanvas(c, ctx, H);
  ctx.clearRect(0, 0, w, H);
  drawAxis(ctx, w, H, t0, t1);
  const X = (t: number) => Math.max(0, ((t - t0) / win) * w);
  const { host: pts, buckets, maxHost } = chartWindow.value;
  const last = pts[pts.length - 1];
  const pool = last?.tokens ?? store.tokens;
  let top = Math.max(1, store.cores, pool, maxHost);
  for (const p of pts) top = Math.max(top, p.hostCores);
  const Y = (v: number) => H - AXIS - (v / top) * (H - AXIS - TOP_PAD);
  const bw = Math.max(1, w / store.windowS);

  let lastX = -Infinity;
  for (const p of pts) {
    const x = X(p.t);
    if (x - lastX < 1) continue;
    lastX = x;
    for (const b of stack(p.byFamily, store.families)) {
      const y = Y(b.y1);
      const h = Y(b.y0) - y;
      ctx.fillStyle = familyColor(b.family);
      ctx.globalAlpha = b.managed ? 1 : 0.5;
      ctx.fillRect(x - bw, y, bw + 0.5, h);
      ctx.globalAlpha = 1;
      if (!b.managed && stripes) {
        ctx.fillStyle = stripes;
        ctx.fillRect(x - bw, y, bw + 0.5, h);
      }
    }
  }

  ctx.strokeStyle = '#222';
  ctx.lineWidth = 1.2;
  drawRecorded(ctx, buckets, 'host_cores', X, Y);
  drawExact(ctx, pts, X, (p) => Y(p.hostCores));

  if (pool > 0) {
    ctx.strokeStyle = '#c4302b';
    ctx.setLineDash([5, 4]);
    drawExact(ctx, pts, X, (p) => Y(p.tokens), true);
    ctx.setLineDash([]);
  }

  ctx.font = FONT;
  ctx.textBaseline = 'top';
  ctx.fillStyle = '#666';
  ctx.fillText(`${top.toFixed(0)} cores · line: all processes on the host · dashed: pool ${pool}`, 4, 2);
  if (buckets.length) {
    ctx.fillStyle = RECORDED_CPU;
    ctx.fillText('10 s recorded averages · all processes', 4, 16);
  } else if (!pts.length) ctx.fillText(store.historyLoaded ? 'no samples in this window' : 'loading history…', 4, 18);
}

let stop = (): void => undefined;
onMounted(() => {
  const c = cv.value;
  const ctx = c?.getContext('2d');
  if (!c || !ctx) return;
  const stripes = hatch(ctx);
  stop = animate(() => draw(c, ctx, stripes));
});
onUnmounted(() => stop());
</script>

<template>
  <canvas ref="cv" role="img" aria-label="Host CPU by tool family and recorded host totals" style="height: 160px"></canvas>
</template>
