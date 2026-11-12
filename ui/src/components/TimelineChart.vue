<script setup lang="ts">
import { onMounted, onUnmounted, reactive, useTemplateRef } from 'vue';
import type { Job } from '../generated/types.ts';
import { AXIS, FONT, animate, drawAxis, sizeCanvas } from '../canvas.ts';
import { tcColor } from '../lib/colors.ts';
import { barLabel, jobId, times, tooltipText } from '../lib/job.ts';
import { assignLanes } from '../lib/lanes.ts';
import { jobs, live, now, select, store } from '../store.ts';

const BAR = 16;
const GAP = 3;
const MIN_H = 80;

interface Bar {
  job: Job;
  x0: number;
  x1: number;
  y: number;
}

const cv = useTemplateRef<HTMLCanvasElement>('cv');
const tip = reactive({ show: false, text: '', x: 0, y: 0 });
let bars: Bar[] = [];

function draw(c: HTMLCanvasElement, ctx: CanvasRenderingContext2D): void {
  const win = store.windowS * 1000;
  const t1 = now();
  const t0 = t1 - win;
  const visible = [...jobs.values()].filter((j) => {
    const t = times(j);
    return (t.ended ?? t1) >= t0 && t.queued <= t1;
  });
  const { placed, lanes } = assignLanes(visible, (j) => times(j).queued, (j) => times(j).ended ?? Infinity);
  const h = Math.max(MIN_H, lanes * (BAR + GAP) + AXIS + 8);
  const w = sizeCanvas(c, ctx, h);
  ctx.clearRect(0, 0, w, h);
  drawAxis(ctx, w, h, t0, t1);
  const X = (t: number) => Math.max(0, ((t - t0) / win) * w);
  const blink = Math.floor(t1 / 500) % 2 === 1;
  const next: Bar[] = [];
  ctx.textBaseline = 'middle';
  ctx.font = FONT;
  for (const { item: j, lane } of placed) {
    const y = 4 + lane * (BAR + GAP);
    const { queued, started } = times(j);
    const end = times(j).ended ?? t1;
    const col = tcColor(j.toolchain);
    ctx.globalAlpha = 0.25;
    ctx.fillStyle = col;
    ctx.fillRect(X(queued), y, Math.max(1, X(started ?? end) - X(queued)), BAR);
    ctx.globalAlpha = 1;
    if (started !== null) {
      const x0 = X(started);
      const x1 = X(end);
      const bw = Math.max(2, x1 - x0);
      ctx.fillStyle = col;
      ctx.fillRect(x0, y, bw, BAR);
      if (j.state === 'running') {
        const l = live.get(jobId(j));
        const frac = l ? Math.min(1, l.cores / Math.max(1, store.tokens)) : 0;
        ctx.fillStyle = 'rgba(255,255,255,0.35)';
        ctx.fillRect(x0, y + BAR - 3, bw * frac, 3);
        ctx.fillStyle = blink ? '#000' : col;
        ctx.fillRect(x1 - 2, y, 2, BAR);
      }
      if (j.state === 'failed') {
        ctx.fillStyle = '#b00';
        ctx.fillRect(x1 - 3, y, 3, BAR);
      }
      if (x1 - x0 > 60) {
        ctx.save();
        ctx.beginPath();
        ctx.rect(x0, y, x1 - x0, BAR);
        ctx.clip();
        ctx.fillStyle = '#fff';
        ctx.fillText(barLabel(j), x0 + 4, y + BAR / 2);
        ctx.restore();
      }
    }
    next.push({ job: j, x0: X(queued), x1: Math.max(X(queued) + 3, X(end)), y });
  }
  ctx.fillStyle = '#333';
  ctx.fillRect(w - 1, 0, 1, h - AXIS + 4);
  bars = next;
}

function hit(ev: MouseEvent): Bar | undefined {
  const c = cv.value;
  if (!c) return undefined;
  const r = c.getBoundingClientRect();
  const x = ev.clientX - r.left;
  const y = ev.clientY - r.top;
  return bars.find((b) => x >= b.x0 - 2 && x <= b.x1 + 2 && y >= b.y && y <= b.y + BAR);
}

function onMove(ev: MouseEvent): void {
  const b = hit(ev);
  if (cv.value) cv.value.style.cursor = b ? 'pointer' : 'default';
  if (!b) {
    tip.show = false;
    return;
  }
  tip.text = tooltipText(b.job, live.get(jobId(b.job)), now());
  tip.x = Math.min(ev.clientX + 14, window.innerWidth - 570);
  tip.y = ev.clientY + 14;
  tip.show = true;
}

function onClick(ev: MouseEvent): void {
  const b = hit(ev);
  if (b) select(jobId(b.job));
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
  <canvas ref="cv" style="height: 80px" @mousemove="onMove" @mouseleave="tip.show = false" @click="onClick"></canvas>
  <div v-show="tip.show" class="tip" :style="{ left: tip.x + 'px', top: tip.y + 'px' }">{{ tip.text }}</div>
</template>

<style scoped>
.tip { position: fixed; pointer-events: none; background: #111; color: #fff; padding: 6px 8px; border-radius: 4px;
  font: 12px/1.35 ui-monospace, monospace; max-width: 560px; white-space: pre-wrap; z-index: 5; }
</style>
