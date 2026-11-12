<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useTemplateRef, watch } from 'vue';
import { fmtDur } from '../lib/format.ts';
import { HEADROOM_PRESETS, HEADROOM_STEP, MAX_HEADROOM, headroomCaption } from '../lib/headroom.ts';
import { fifoQueue, queueSummary, waitStats } from '../lib/queue.ts';
import { census, jobs, setHeadroom, store } from '../store.ts';
import ToolDot from './ToolDot.vue';

const WINDOWS: ReadonlyArray<[number, string]> = [
  [120, '2 min'],
  [600, '10 min'],
  [1800, '30 min'],
  [3600, '1 h'],
  [21600, '6 h'],
];

const limit = computed(() => store.limit || store.tokens);
const used = computed(() => Math.max(0, limit.value - store.free));

/** Slider value; follows the daemon except while the user moves it. */
const draft = ref(store.headroom);
let editedAt = 0;
watch(
  () => store.headroom,
  (v) => {
    if (Date.now() - editedAt > 1500) draft.value = v;
  },
);
// A refused change puts the slider back on what the daemon runs, so the caption never claims it.
watch(
  () => store.headroomRefused,
  () => {
    draft.value = store.headroom;
  },
);
function pick(percent: number): void {
  editedAt = Date.now();
  draft.value = percent;
  setHeadroom(percent);
}
const onHeadroom = (e: Event): void => pick(Number((e.target as HTMLInputElement).value));
const caption = computed(() => headroomCaption(store.cores || store.tokens, store.tokens, draft.value));
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
    <span class="conn" data-ux-region="connection" :class="{ ok: store.connected }" role="img"
      :aria-label="store.connected ? 'live: connected to the daemon' : 'not connected to the daemon'"
      :title="store.connected ? 'live: connected to the daemon' : 'not connected to the daemon'"></span>
    <span v-if="store.error" class="bad">{{ store.error }}</span>
    <span data-ux-region="slots" :title="limit < store.tokens ? `${store.tokens - limit} slot(s) withheld for headroom` : 'slots in use'">slots
      <span class="tokens"><i v-for="k in store.tokens" :key="k" :class="{ on: k <= used, off: k > limit }"></i></span>
      <b>{{ used }}/{{ limit }}</b></span>
    <span class="headroom" data-ux-region="headroom">
      <label title="Share of the CPU kept free for you (browser, video calls). Builds get the rest: fewer slots, and a CPU quota on all build jobs together.">
        headroom for you
        <input type="range" min="0" :max="MAX_HEADROOM" :step="HEADROOM_STEP" :value="draft"
          aria-describedby="headroom-caption" @input="onHeadroom" /></label>
      <b>{{ draft }}%</b>
      <span class="presets" role="group" aria-label="headroom presets">
        <button v-for="p in HEADROOM_PRESETS" :key="p" type="button" :aria-pressed="draft === p" @click="pick(p)">{{ p }} %</button>
      </span>
      <span id="headroom-caption" class="muted">{{ caption }}</span></span>
    <span v-if="store.callApps.length" class="call"
      title="Call mode: a call uses the microphone or the camera, so the headroom is raised; it goes back 2 min after the call. Moving the slider now keeps your value.">
      call · {{ store.callApps.join(', ') }}</span>
    <span v-if="store.headroomError" class="bad">{{ store.headroomError }}</span>
    <span>running <b>{{ running }}</b></span>
    <span data-ux-region="queue">queue <b>{{ queue.length }}</b> · longest wait <b>{{ fmtDur(queue.longestS) }}</b></span>
    <span :title="`slot wait (start - enqueue) of ${waits.n} jobs finished in the last 60 min; gave up = cancelled while queued`">
      wait p50 <b>{{ pct(waits.p50) }}</b> · p90 <b>{{ pct(waits.p90) }}</b> · gave up <b>{{ waits.gaveUp }}</b>
      <span class="muted">(60 min)</span></span>
    <span>job cores <b>{{ store.jobCores.toFixed(1) }}</b></span>
    <span>host CPU <b>{{ (store.sysCpu * 100).toFixed(0) }}</b>%</span>
    <span v-if="census" title="CPU pressure (some avg10); cores of builds slot does not run; pool size the controller recommends, not enforced">
      PSI cpu <b>{{ census.psi_cpu.toFixed(0) }}</b>% · unmanaged builds <b>{{ census.unmanaged_cores.toFixed(1) }}</b> cores
      · recommended <b>{{ census.recommended_tokens }}/{{ store.tokens }}</b> <span class="muted">(observe)</span></span>
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
.tokens i.off { background: transparent; outline: 1px dashed #bbb; outline-offset: -1px; }
.headroom { display: inline-flex; gap: 6px; align-items: center; }
.headroom label { display: inline-flex; gap: 6px; align-items: center; }
.headroom input { width: 110px; }
.call { padding: 1px 7px; border-radius: 9px; background: #e6f0ff; color: #1d4f91; }
.presets { display: inline-flex; gap: 2px; }
.presets button { min-width: 24px; min-height: 24px; padding: 0 6px; border: 1px solid #bbb; border-radius: 3px; background: #fff;
  color: var(--fg); cursor: pointer; font-variant-numeric: tabular-nums; }
.presets button[aria-pressed='true'] { background: #444; border-color: #444; color: #fff; }
.conn { width: 9px; height: 9px; border-radius: 50%; display: inline-block; background: #c33; }
.conn.ok { background: #2a2; }
.legend > span { margin-right: 12px; }
</style>
