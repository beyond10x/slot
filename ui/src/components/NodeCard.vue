<script setup lang="ts">
import { computed, onMounted, useTemplateRef, watch } from 'vue';
import type { Job, Node } from '../generated/types.ts';
import { tcColor } from '../lib/colors.ts';
import {
  cpuOf, failText, failedOf, fmtGib, fmtLeft, hostLine, laneColor, leaseMismatch, lifeLeft, nodeCounts, slotCells, specLine, stateText as stateWords,
} from '../lib/cluster.ts';
import type { NodeJobs } from '../lib/cluster.ts';
import { jobs as allJobs, nodeSparks, store } from '../store.ts';

const props = defineProps<{ node: Node; name: string; running: Job[]; jobs: NodeJobs; pressed: boolean; dim: boolean; dark: boolean }>();
const emit = defineEmits<{ toggle: [] }>();

/** The cells shown: at most 64, four rows of 16; a larger node says how many more it has. */
const MAX_CELLS = 64;
const cells = computed(() => slotCells(Math.min(props.node.slots, MAX_CELLS), props.running));
const hiddenSlots = computed(() => Math.max(0, props.node.slots - MAX_CELLS));
const stateText = computed(() => stateWords(props.node));
const stateClass = computed(() => props.node.state.toLowerCase());
const kindText = computed(() => props.node.kind.toLowerCase());
/** The poweroff the node has scheduled against the deadline of the cluster file: a text when they disagree. */
const mismatch = computed(() => leaseMismatch(props.node, store.clock));
const life = computed(() => {
  const l = lifeLeft(props.node, store.clock);
  if (!l) return null;
  const r = 22;
  const c = 2 * Math.PI * r;
  return {
    c, offset: c * (1 - l.frac), label: fmtLeft(l.leftS), warn: props.node.state === 'Draining' || l.frac < 0.25,
    caption: `left of ${l.spanH >= 1 ? Math.round(l.spanH) + ' h' : Math.round(l.spanH * 60) + ' min'}`,
    title: mismatch.value?.text ?? (props.node.poweroff_at ? 'until the poweroff the node has scheduled' : 'until the deadline of the cluster file'),
  };
});
/** The node's finished jobs over the last 60 min and today, as it counts them; failures by cause. */
const counts = computed(() => {
  const c = nodeCounts(props.node, allJobs.values(), store.clock);
  const words = (o: typeof c.hour): string =>
    `${o.ok.toLocaleString('en')} done, ${failedOf(o).toLocaleString('en')} failed${failText(o) ? ` (${failText(o)})` : ''}`;
  return {
    ...c, hourFailed: failedOf(c.hour), todayFailed: failedOf(c.today), hourWhy: failText(c.hour),
    title: `last 60 min: ${words(c.hour)}; today: ${words(c.today)}`,
  };
});
/** The whole machine's CPUs and load: what the owner reads for "is this machine busy". */
const host = computed(() => hostLine(props.node));
const shownWts = computed(() => (props.jobs.checkouts.length > 3 ? props.jobs.checkouts.slice(0, 2) : props.jobs.checkouts));
const moreWts = computed(() => props.jobs.checkouts.slice(shownWts.value.length));
const memUsedGib = computed(() => (props.node.memory_used_mib ?? 0) / 1024);
const diskPct = computed(() => (props.node.disk_gib > 0 ? (100 * (props.node.disk_free_gib ?? 0)) / props.node.disk_gib : 100));
const memPct = computed(() => (props.node.memory_gib > 0 ? (100 * memUsedGib.value) / props.node.memory_gib : 0));
const label = computed(
  () => `${props.name}, ${kindText.value} node, ${stateText.value}, ${specLine(props.node)}, ${props.node.slots_in_use} of ${props.node.slots} slots busy.${mismatch.value ? ` Lease in doubt: ${mismatch.value.text}.` : ''} Press to show only this node's jobs.`,
);

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault();
    emit('toggle');
  }
}

const canvas = useTemplateRef<HTMLCanvasElement>('spark');
/** The busy CPUs of the last 60 s as an area under a line, scaled to the node's CPUs. */
function drawSpark(): void {
  const cv = canvas.value;
  if (!cv) return;
  const w = cv.clientWidth;
  const h = cv.clientHeight;
  if (!w || !h) return;
  const dpr = window.devicePixelRatio || 1;
  if (cv.width !== Math.round(w * dpr)) {
    cv.width = Math.round(w * dpr);
    cv.height = Math.round(h * dpr);
  }
  const g = cv.getContext('2d');
  if (!g) return;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, w, h);
  const css = getComputedStyle(cv);
  const line = css.getPropertyValue('--line').trim() || '#888';
  const color = css.getPropertyValue('--accent').trim() || '#1f5fbf';
  g.strokeStyle = line;
  g.lineWidth = 1;
  g.beginPath();
  g.moveTo(0, h - 0.5);
  g.lineTo(w, h - 0.5);
  g.stroke();
  const pts = nodeSparks.get(props.node.node_id);
  if (pts.length < 2) return;
  const t1 = store.clock;
  const t0 = t1 - 60_000;
  const max = Math.max(1, props.node.cpus);
  const xy = pts.filter((p) => p.t >= t0 - 2000).map((p) => [Math.max(0, ((p.t - t0) / 60_000) * w), h - 2 - (h - 4) * Math.min(1, p.v / max)] as const);
  if (xy.length < 2) return;
  g.beginPath();
  g.moveTo(xy[0][0], h);
  for (const [x, y] of xy) g.lineTo(x, y);
  g.lineTo(xy[xy.length - 1][0], h);
  g.closePath();
  g.globalAlpha = props.dark ? 0.35 : 0.22;
  g.fillStyle = color;
  g.fill();
  g.globalAlpha = 1;
  g.beginPath();
  xy.forEach(([x, y], i) => (i ? g.lineTo(x, y) : g.moveTo(x, y)));
  g.strokeStyle = color;
  g.lineWidth = 1.5;
  g.stroke();
  const [lx, ly] = xy[xy.length - 1];
  g.beginPath();
  g.arc(Math.min(w - 3, lx), ly, 2.5, 0, Math.PI * 2);
  g.fillStyle = color;
  g.fill();
}
onMounted(drawSpark);
watch(() => [store.clock, props.node, props.dark], drawSpark, { flush: 'post' });
</script>

<template>
  <div class="node" :class="[stateClass, { dim, mismatch }]" role="button" tabindex="0" :aria-pressed="pressed" :title="label" :data-node="node.node_id"
    @click="emit('toggle')" @keydown="onKey">
    <div class="nhead">
      <span class="nname" :title="name">{{ name }}</span><span class="tag">{{ kindText }}</span>
      <span class="pill" :class="stateClass">{{ stateText }}</span>
      <span class="spec" :title="specLine(node)">{{ specLine(node) }}</span>
      <span class="ncount" :title="counts.title" data-counts>slots <b>{{ node.slots_in_use }}</b>/{{ node.slots }} ·
        60 min <b>{{ counts.hour.ok.toLocaleString('en') }}</b> done<template v-if="counts.hourFailed">,
          <span class="f">{{ counts.hourFailed.toLocaleString('en') }} failed</span></template> ·
        today <b>{{ counts.today.ok.toLocaleString('en') }}</b> done<template v-if="counts.todayFailed">,
          <span class="f">{{ counts.todayFailed.toLocaleString('en') }} failed</span></template></span>
    </div>
    <div class="nbody">
      <div class="slots" aria-hidden="true">
        <i v-for="(c, k) in cells" :key="k" class="cell" :class="{ busy: c }" :data-cell-job="c?.id"
          :style="c ? { background: tcColor(c.tool) } : undefined"></i>
        <span v-if="hiddenSlots" class="more">+{{ hiddenSlots }}</span>
      </div>
      <div class="spark" :class="{ why: counts.hourWhy }">
        <span v-if="host" class="lab" data-host :title="`${host}; the pool's jobs ${cpuOf(node).toFixed(1)} CPUs`">{{ host }}</span>
        <span v-else class="lab">CPU <b>{{ cpuOf(node).toFixed(1) }}</b> of {{ node.cpus }} · 60 s</span>
        <canvas ref="spark" aria-hidden="true"></canvas>
        <span v-if="counts.hourWhy" class="lab why" data-fail-causes :title="`failed in the last 60 min: ${counts.hourWhy}`">failed: {{ counts.hourWhy }}</span>
      </div>
      <div class="bars">
        <div><span>memory <template v-if="node.memory_gib"><b>{{ Math.round(memUsedGib) }}</b> / {{ node.memory_gib }} GiB</template>
          <template v-else>unknown</template></span>
          <div class="bar mem"><i :style="{ width: memPct.toFixed(1) + '%' }"></i></div></div>
        <div><span>disk free <b>{{ node.disk_free_gib === undefined ? '–' : fmtGib(node.disk_free_gib) }}</b></span>
          <div class="bar disk" :class="{ low: diskPct < 20 }"><i :style="{ width: diskPct.toFixed(1) + '%' }"></i></div></div>
      </div>
      <div class="wts">
        <div class="h">{{ jobs.checkouts.length ? `${jobs.checkouts.length} ${jobs.checkouts.length === 1 ? 'checkout' : 'checkouts'} with jobs` : 'no job here now' }}</div>
        <div v-for="w in shownWts" :key="w" :title="w"><i :style="{ background: laneColor(w, dark) }"></i>{{ w }}</div>
        <div v-if="moreWts.length" class="h" :title="moreWts.join(', ')">+{{ moreWts.length }} more</div>
      </div>
      <div class="life">
        <template v-if="life">
          <svg viewBox="0 0 52 52" aria-hidden="true">
            <circle class="trk" cx="26" cy="26" r="22" fill="none" stroke-width="5" />
            <circle class="arc" :class="{ warn: life.warn, bad: mismatch }" cx="26" cy="26" r="22" fill="none" stroke-width="5" stroke-linecap="round"
              :stroke-dasharray="life.c.toFixed(2)" :stroke-dashoffset="life.offset.toFixed(2)" transform="rotate(-90 26 26)" />
            <text x="26" y="29.5" text-anchor="middle">{{ life.label }}</text>
          </svg>
          <span v-if="mismatch" class="lease" role="status" :title="mismatch.text" :aria-label="mismatch.text" data-lease-mismatch>{{ mismatch.short }}</span>
          <span v-else :title="life.title">{{ life.caption }}</span>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.node { position: absolute; inset: 0; display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: 10px; background: var(--panel);
  padding: 7px 12px 8px; cursor: pointer; transition: opacity .6s, border-color .4s, box-shadow .3s; }
.node:hover { border-color: var(--ctl); }
.node[aria-pressed='true'] { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent), 0 0 24px -6px color-mix(in srgb, var(--accent) 60%, transparent); }
.node.dim { opacity: .38; }
.node.draining { border-style: dashed; border-color: var(--warn); }
.node.joining { border-style: dashed; border-color: var(--accent); }
.node.mismatch { border-color: var(--bad); box-shadow: 0 0 0 1px var(--bad); }
.nhead { display: flex; align-items: center; gap: 8px; height: 22px; white-space: nowrap; min-width: 0; }
.nname { font-weight: 700; font-size: 14px; max-width: 180px; flex: none; overflow: hidden; text-overflow: ellipsis; }
.tag { font-size: 11px; padding: 0 6px; border-radius: 9px; border: 1px solid var(--ctl); color: var(--mute); line-height: 16px; }
.pill { font-size: 12px; padding: 0 8px; border-radius: 9px; line-height: 18px; font-weight: 600; }
.pill.ready { background: color-mix(in srgb, var(--ok) 18%, transparent); color: var(--ok); }
.pill.draining { background: color-mix(in srgb, var(--warn) 20%, transparent); color: var(--warn); }
.pill.joining { background: color-mix(in srgb, var(--accent) 20%, transparent); color: var(--accent); }
.pill.gone { color: var(--mute); }
.spec { font-size: 12px; color: var(--mute); overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.ncount { margin-left: auto; font-size: 12px; color: var(--mute); font-variant-numeric: tabular-nums; flex: none; }
.ncount b { color: var(--fg); font-weight: 600; }
.ncount .f { color: var(--bad); font-weight: 600; }
.nbody { flex: 1; display: grid; grid-template-columns: 271px minmax(80px, 1fr) 138px 158px 60px; gap: 16px; align-items: center; min-height: 0;
  margin-top: 4px; }
.slots { position: relative; display: grid; grid-template-columns: repeat(16, 14px); gap: 3px; align-content: center; }
.cell { width: 14px; height: 14px; border-radius: 3px; background: var(--cell); transition: background-color .45s; }
.more { position: absolute; right: 0; bottom: -15px; font-size: 11px; color: var(--mute); }
.node.draining .cell:not(.busy) { background: repeating-linear-gradient(135deg, var(--cell) 0 3px, transparent 3px 6px); }
.spark { position: relative; height: 100%; min-width: 0; display: flex; flex-direction: column; justify-content: center; }
.lab { font-size: 12px; color: var(--mute); line-height: 15px; white-space: nowrap; }
.lab b { color: var(--fg); font-weight: 600; font-variant-numeric: tabular-nums; }
.spark canvas { width: 100%; height: 44px; display: block; margin-top: 3px; }
/* With the line of failure causes under it the chart gives up the room of that line. */
.spark.why canvas { height: 27px; }
.bars { display: flex; flex-direction: column; gap: 8px; font-size: 12px; color: var(--mute); line-height: 15px; white-space: nowrap; }
.bars b { color: var(--fg); font-weight: 600; font-variant-numeric: tabular-nums; }
.bar { height: 5px; border-radius: 3px; background: var(--track); overflow: hidden; margin-top: 2px; }
.bar i { display: block; height: 100%; border-radius: 3px; background: var(--queue); transition: width .8s ease; }
.bar.disk i { background: var(--accent); }
.bar.disk.low i { background: var(--warn); }
.wts { font-size: 12px; line-height: 16px; min-width: 0; color: var(--fg); }
.wts .h { color: var(--mute); }
.wts div { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.wts i { display: inline-block; width: 8px; height: 8px; border-radius: 50%; margin-right: 5px; }
.life { display: flex; flex-direction: column; align-items: center; gap: 1px; font-size: 11px; color: var(--mute); line-height: 13px; white-space: nowrap; }
.life svg { width: 52px; height: 52px; display: block; }
.life .trk { stroke: var(--track); }
.life .arc { stroke: var(--ok); transition: stroke-dashoffset .9s linear; }
.life .arc.warn { stroke: var(--warn); }
.life .arc.bad { stroke: var(--bad); }
.life .lease { color: var(--bad); font-weight: 600; }
.spark .why { color: var(--bad); overflow: hidden; text-overflow: ellipsis; margin-top: 2px; }
.life text { font: 600 10.5px var(--mono); fill: var(--fg); }
@media (max-width: 1330px) {
  .nbody { grid-template-columns: 271px minmax(70px, 1fr) 124px 140px 56px; gap: 12px; }
}
/* In the stacked page (below 1100 px) the card takes the height it needs: its head may wrap, memory, disk and checkouts go to a second row. */
@media (max-width: 1099px) {
  .nhead { flex-wrap: wrap; height: auto; min-height: 22px; row-gap: 2px; }
  .nname { max-width: none; }
  .nbody { grid-template-columns: 271px minmax(0, 1fr) 56px; grid-template-areas: 'slots spark life' 'bars wts life'; gap: 10px 12px; }
  .slots { grid-area: slots; }
  .spark { grid-area: spark; height: 64px; }
  .bars { grid-area: bars; }
  .wts { grid-area: wts; }
  .life { grid-area: life; }
}
@media (prefers-reduced-motion: reduce) {
  .node, .cell, .bar i, .life .arc { transition-property: opacity, background-color; }
}
</style>
