<script setup lang="ts">
import { onMounted, onUnmounted, useTemplateRef } from 'vue';
import { AXIS, FONT, animate, drawAxis, sizeCanvas } from '../canvas.ts';
import type { HostSample } from '../lib/host.ts';
import { hostSeries, now, store } from '../store.ts';

export interface Line {
  label: string;
  color: string;
  value: (s: HostSample) => number;
  step?: boolean;
  dashed?: boolean;
}

const props = defineProps<{
  lines: readonly Line[];
  unit: string;
  /** Smallest value the top of the scale may take. */
  minTop: number;
  /** Largest value the top of the scale may take. */
  maxTop?: number;
}>();

const H = 80;
const TOP_PAD = 14;

const cv = useTemplateRef<HTMLCanvasElement>('cv');

function draw(c: HTMLCanvasElement, ctx: CanvasRenderingContext2D): void {
  const win = store.windowS * 1000;
  const t1 = now();
  const t0 = t1 - win;
  const w = sizeCanvas(c, ctx, H);
  ctx.clearRect(0, 0, w, H);
  drawAxis(ctx, w, H, t0, t1);
  const X = (t: number) => Math.max(0, ((t - t0) / win) * w);
  const pts = hostSeries.filter((p) => p.t >= t0 - 2000);
  let top = props.minTop;
  for (const p of pts) for (const l of props.lines) top = Math.max(top, l.value(p));
  top = Math.ceil(Math.min(top, props.maxTop ?? Infinity));
  const Y = (v: number) => H - AXIS - (Math.min(v, top) / top) * (H - AXIS - TOP_PAD);

  ctx.lineWidth = 1.4;
  for (const l of props.lines) {
    ctx.strokeStyle = l.color;
    ctx.setLineDash(l.dashed ? [5, 4] : []);
    ctx.beginPath();
    let prevY = 0;
    pts.forEach((p, i) => {
      const x = X(p.t);
      const y = Y(l.value(p));
      if (!i) ctx.moveTo(x, y);
      else {
        if (l.step) ctx.lineTo(x, prevY);
        ctx.lineTo(x, y);
      }
      prevY = y;
    });
    ctx.stroke();
  }
  ctx.setLineDash([]);
  ctx.font = FONT;
  ctx.textBaseline = 'top';
  ctx.fillStyle = '#666';
  ctx.fillText(`${top} ${props.unit}`, 4, 1);
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
  <div>
    <canvas ref="cv" style="height: 80px"></canvas>
    <div class="legend">
      <span v-for="l in lines" :key="l.label">
        <i :class="{ dashed: l.dashed }" :style="{ borderColor: l.color }"></i>{{ l.label }}
      </span>
    </div>
  </div>
</template>

<style scoped>
.legend { display: flex; flex-wrap: wrap; gap: 2px 12px; font-size: 12px; color: var(--mute); margin-top: 2px; }
.legend i { display: inline-block; width: 14px; height: 0; border-top: 2px solid; vertical-align: middle; margin-right: 4px; }
.legend i.dashed { border-top-style: dashed; }
</style>
