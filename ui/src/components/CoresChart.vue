<script setup lang="ts">
import { onMounted, onUnmounted, useTemplateRef } from 'vue';
import { AXIS, FONT, animate, drawAxis, sizeCanvas } from '../canvas.ts';
import { QUEUE_COLOR, stackOrder, tcColor } from '../lib/colors.ts';
import { now, series, store } from '../store.ts';

const H = 150;
/** Height of the queue-length strip between the cores area and the time axis. */
const QH = 30;
const CORES_BOTTOM = H - AXIS - QH - 6;
const QUEUE_TOP = CORES_BOTTOM + 6;
const QUEUE_BOTTOM = H - AXIS;

const cv = useTemplateRef<HTMLCanvasElement>('cv');

function draw(c: HTMLCanvasElement, ctx: CanvasRenderingContext2D): void {
  const win = store.windowS * 1000;
  const t1 = now();
  const t0 = t1 - win;
  const w = sizeCanvas(c, ctx, H);
  ctx.clearRect(0, 0, w, H);
  drawAxis(ctx, w, H, t0, t1);
  const X = (t: number) => Math.max(0, ((t - t0) / win) * w);
  const top = Math.max(store.cores || store.tokens || 1, 1);
  const Y = (v: number) => CORES_BOTTOM - (v / top) * (CORES_BOTTOM - 6);
  const pts = series.filter((p) => p.t >= t0 - 2000);
  const tools = stackOrder(pts.flatMap((p) => Object.keys(p.by)));
  const bw = Math.max(1, w / store.windowS);

  ctx.fillStyle = '#ddd';
  ctx.fillRect(0, CORES_BOTTOM, w, 1);
  ctx.fillRect(0, QUEUE_BOTTOM, w, 1);

  for (const p of pts) {
    let acc = 0;
    const x = X(p.t);
    for (const k of tools) {
      const v = p.by[k] ?? 0;
      if (!v) continue;
      ctx.fillStyle = tcColor(k);
      ctx.fillRect(x - bw, Y(acc + v), bw, Y(acc) - Y(acc + v));
      acc += v;
    }
  }
  ctx.strokeStyle = '#222';
  ctx.lineWidth = 1.2;
  ctx.beginPath();
  pts.forEach((p, i) => {
    if (i) ctx.lineTo(X(p.t), Y(p.sys));
    else ctx.moveTo(X(p.t), Y(p.sys));
  });
  ctx.stroke();

  const qmax = Math.max(1, ...pts.map((p) => p.waiting));
  const QY = (v: number) => QUEUE_BOTTOM - (v / qmax) * (QH - 12);
  ctx.fillStyle = QUEUE_COLOR;
  for (const p of pts) {
    if (!p.waiting) continue;
    const x = X(p.t);
    ctx.fillRect(x - bw, QY(p.waiting), bw, QUEUE_BOTTOM - QY(p.waiting));
  }

  ctx.font = FONT;
  ctx.textBaseline = 'top';
  ctx.fillStyle = '#666';
  ctx.fillText(top + ' cores', 4, 2);
  ctx.fillText('line: host CPU in use', 70, 2);
  if (!pts.length) ctx.fillText(store.historyLoaded ? 'no samples in this window' : 'loading history…', 4, 18);
  ctx.fillStyle = QUEUE_COLOR;
  ctx.fillRect(4, QUEUE_TOP + 2, 9, 9);
  ctx.fillStyle = '#666';
  ctx.fillText(`queue length (max ${qmax})`, 18, QUEUE_TOP + 1);
}

let stop = (): void => undefined;
onMounted(() => {
  const c = cv.value;
  const ctx = c?.getContext('2d');
  if (c && ctx) stop = animate(() => draw(c, ctx));
});
onUnmounted(() => stop());
</script>

<template>
  <canvas ref="cv" style="height: 150px"></canvas>
</template>
