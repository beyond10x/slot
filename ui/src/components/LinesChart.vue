<script lang="ts">
import type { HostSample } from '../lib/host.ts';

export interface Line<P = HostSample> {
  label: string;
  color: string;
  /** The line's height at a point; a point without one (`NaN`) leaves a hole. */
  value: (s: P) => number;
  step?: boolean;
  dashed?: boolean;
  /** The newest value in words, shown in the legend after the label. */
  now?: () => string;
}
</script>

<script setup lang="ts" generic="P extends { t: number }">
import { onMounted, onUnmounted, useTemplateRef } from 'vue';
import { AXIS, FONT, INK, animate, drawAxis, drawExact, drawSpans, sizeCanvas } from '../canvas.ts';
import { now, store } from '../store.ts';

const props = defineProps<{
  lines: readonly Line<P>[];
  /** The points in the window, in the order of time; read on every frame. */
  points: () => readonly P[];
  unit: string;
  /** Smallest value the top of the scale may take. */
  minTop: number;
  /** Largest value the top of the scale may take. */
  maxTop?: number;
  /** Each point stands for this many ms that end at its `t`; without it a point is one tick of a second. */
  span?: number;
  /** Said in place of the lines when there is nothing to draw; the chart keeps its height. */
  empty?: string;
  /** More about the newest values, shown on hover. */
  note?: string;
}>();

/** The plot's height where the page sets none (`--plot-h`). */
const HEIGHT = 80;
const TOP_PAD = 14;

const cv = useTemplateRef<HTMLCanvasElement>('cv');
const plot = useTemplateRef<HTMLDivElement>('plot');

function draw(c: HTMLCanvasElement, ctx: CanvasRenderingContext2D): void {
  const win = store.windowS * 1000;
  const t1 = now();
  const t0 = t1 - win;
  // The height is the page's: a row of charts makes its plots lower where its legends take a second line.
  const H = plot.value?.clientHeight || HEIGHT;
  const w = sizeCanvas(c, ctx, H);
  ctx.clearRect(0, 0, w, H);
  drawAxis(ctx, w, H, t0, t1);
  const X = (t: number) => Math.max(0, ((t - t0) / win) * w);
  const pts = props.points();
  let top = props.minTop;
  for (const p of pts) {
    for (const l of props.lines) {
      const v = l.value(p);
      if (v > top) top = v;
    }
  }
  top = Math.ceil(Math.min(top, props.maxTop ?? Infinity));
  const Y = (v: number) => H - AXIS - (Math.min(v, top) / top) * (H - AXIS - TOP_PAD);

  ctx.lineWidth = 1.4;
  for (const l of props.lines) {
    ctx.strokeStyle = l.color;
    ctx.setLineDash(l.dashed ? [5, 4] : []);
    if (props.span) drawSpans(ctx, pts, X, (p) => Y(l.value(p)), props.span);
    else drawExact(ctx, pts, X, (p) => Y(l.value(p)), l.step);
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
  <div class="lines" :title="note || undefined">
    <div ref="plot" class="plot">
      <canvas ref="cv"></canvas>
      <p v-if="empty" class="none"><span>{{ empty }}</span></p>
    </div>
    <div class="legend">
      <span v-for="l in lines" :key="l.label">
        <i :class="{ dashed: l.dashed }" :style="{ borderColor: l.color }"></i>{{ l.label }}<b v-if="l.now?.()">{{ l.now() }}</b>
      </span>
    </div>
  </div>
</template>

<style scoped>
/* The height is set here and not by the canvas, so the chart has it before the first frame is drawn. */
.plot { position: relative; height: var(--plot-h, 80px); overflow: hidden; }
/* In the plot area, between the scale's label and the time axis. */
.none { position: absolute; inset: 14px 0 20px; margin: 0; display: flex; align-items: center; justify-content: center;
  color: var(--mute); font-size: 13px; pointer-events: none; }
/* The grid lines of the canvas stop at the words. */
.none span { padding: 0 6px; background: var(--bg); }
.legend i { display: inline-block; width: 14px; height: 0; border-top: 2px solid; vertical-align: middle; margin-right: 5px; }
.legend i.dashed { border-top-style: dashed; }
.legend b { margin-left: 5px; }
</style>
