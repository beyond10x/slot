<script setup lang="ts">
import { onMounted, onUnmounted, useTemplateRef } from 'vue';
import { AXIS, FONT, INK, animate, drawAxis, drawExact, sizeCanvas } from '../canvas.ts';
import type { HostSample } from '../lib/host.ts';
import { chartWindow, now, store } from '../store.ts';

export interface Line {
  label: string;
  color: string;
  value: (s: HostSample) => number;
  step?: boolean;
  dashed?: boolean;
  /** The newest value in words, shown in the legend after the label. */
  now?: () => string;
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
  const pts = chartWindow.value.host;
  let top = props.minTop;
  for (const p of pts) for (const l of props.lines) top = Math.max(top, l.value(p));
  top = Math.ceil(Math.min(top, props.maxTop ?? Infinity));
  const Y = (v: number) => H - AXIS - (Math.min(v, top) / top) * (H - AXIS - TOP_PAD);

  ctx.lineWidth = 1.4;
  for (const l of props.lines) {
    ctx.strokeStyle = l.color;
    ctx.setLineDash(l.dashed ? [5, 4] : []);
    drawExact(ctx, pts, X, (p) => Y(l.value(p)), l.step);
  }
  ctx.setLineDash([]);
  ctx.font = FONT;
  ctx.textBaseline = 'top';
  ctx.fillStyle = INK;
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
        <i :class="{ dashed: l.dashed }" :style="{ borderColor: l.color }"></i>{{ l.label }}<b v-if="l.now?.()">{{ l.now() }}</b>
      </span>
    </div>
  </div>
</template>

<style scoped>
.legend i { display: inline-block; width: 14px; height: 0; border-top: 2px solid; vertical-align: middle; margin-right: 5px; }
.legend i.dashed { border-top-style: dashed; }
.legend b { margin-left: 5px; }
</style>
