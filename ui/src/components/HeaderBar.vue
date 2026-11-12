<script setup lang="ts">
import { computed, onMounted, onUnmounted, useTemplateRef } from 'vue';
import { fmtDur } from '../lib/format.ts';
import { fifoQueue, queueSummary, waitStats } from '../lib/queue.ts';
import { jobs, store } from '../store.ts';
import ToolDot from './ToolDot.vue';

const WINDOWS: ReadonlyArray<[number, string]> = [
  [120, '2 min'],
  [600, '10 min'],
  [1800, '30 min'],
  [3600, '1 h'],
  [21600, '6 h'],
];

const used = computed(() => Math.max(0, store.tokens - store.free));
const running = computed(() => {
  let n = 0;
  for (const j of jobs.values()) if (j.state === 'running') n++;
  return n;
});
const queue = computed(() => queueSummary(fifoQueue(jobs.values()), store.clock));
const waits = computed(() => waitStats(jobs.values(), store.clock));
const pct = (v: number | null): string => (v === null ? '–' : fmtDur(v));

const el = useTemplateRef<HTMLElement>('el');
let ro: ResizeObserver | null = null;
onMounted(() => {
  const h = el.value;
  if (!h) return;
  ro = new ResizeObserver(() => document.documentElement.style.setProperty('--hdr', h.offsetHeight + 'px'));
  ro.observe(h);
});
onUnmounted(() => ro?.disconnect());
</script>

<template>
  <header ref="el">
    <h1>slot · {{ store.host }}<template v-if="store.cores"> · {{ store.cores }} cores</template></h1>
    <span class="conn" :class="{ ok: store.connected }" title="event stream"></span>
    <span v-if="store.error" class="bad">{{ store.error }}</span>
    <span>slots
      <span class="tokens"><i v-for="k in store.tokens" :key="k" :class="{ on: k <= used }"></i></span>
      <b>{{ used }}/{{ store.tokens }}</b></span>
    <span>running <b>{{ running }}</b></span>
    <span>queue <b>{{ queue.length }}</b> · longest wait <b>{{ fmtDur(queue.longestS) }}</b></span>
    <span :title="`slot wait (start - enqueue) of ${waits.n} jobs finished in the last 60 min; gave up = cancelled while queued`">
      wait p50 <b>{{ pct(waits.p50) }}</b> · p90 <b>{{ pct(waits.p90) }}</b> · gave up <b>{{ waits.gaveUp }}</b>
      <span class="muted">(60 min)</span></span>
    <span>job cores <b>{{ store.jobCores.toFixed(1) }}</b></span>
    <span>host CPU <b>{{ (store.sysCpu * 100).toFixed(0) }}</b>%</span>
    <label>window
      <select v-model.number="store.windowS">
        <option v-for="[s, label] in WINDOWS" :key="s" :value="s">{{ label }}</option>
      </select></label>
    <span class="legend"><ToolDot v-for="n in store.toolchains" :key="n" :name="n" /></span>
  </header>
</template>

<style scoped>
header { display: flex; flex-wrap: wrap; gap: 18px; align-items: center; padding: 10px 14px; border-bottom: 1px solid var(--line);
  position: sticky; top: 0; background: var(--bg); z-index: 2; }
h1 { font-size: 15px; margin: 0; }
b { font-variant-numeric: tabular-nums; }
.tokens { display: inline-flex; gap: 2px; vertical-align: middle; }
.tokens i { width: 9px; height: 14px; background: #e3e3e3; border-radius: 2px; }
.tokens i.on { background: #444; }
.conn { width: 9px; height: 9px; border-radius: 50%; display: inline-block; background: #c33; }
.conn.ok { background: #2a2; }
.legend > span { margin-right: 12px; }
</style>
