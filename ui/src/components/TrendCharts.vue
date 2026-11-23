<script setup lang="ts">
import { computed } from 'vue';
import { MINUTE_MS, cachePoint, describeMinute, emptyText, minuteAndDay } from '../lib/cache.ts';
import type { CachePoint } from '../lib/cache.ts';
import type { HostSample } from '../lib/host.ts';
import { cacheChart, cacheDay, census, chartWindow, store } from '../store.ts';
import LinesChart from './LinesChart.vue';
import type { Line } from './LinesChart.vue';
import SectionHead from './SectionHead.vue';

// The line charts of the machine, above the job tables: what the host does comes before who waits for it.
const percent = (v: number | undefined): string => (v === undefined ? '' : `${v.toFixed(0)} %`);
const cores = (v: number | undefined): string => (v === undefined ? '' : v.toFixed(1));

const host = (): readonly HostSample[] => chartWindow.value.host;

const CORES: readonly Line[] = [
  { label: 'all programs', color: '#444444', value: (s) => s.hostCores, now: () => cores(census.value?.host_cores) },
  { label: 'builds', color: '#0072B2', value: (s) => s.buildCores, now: () => cores(census.value?.build_cores) },
  { label: 'not on slot', color: '#a85400', value: (s) => s.unmanagedCores, dashed: true, now: () => cores(census.value?.unmanaged_cores) },
];

// The pressure lines keep clear of red: red is the colour of a failure (TimelineChart.vue, the Finished table).
const PRESSURE: readonly Line[] = [
  { label: 'CPU', color: '#8c6d31', value: (s) => s.psiCpu, now: () => percent(census.value?.psi_cpu) },
  { label: 'memory', color: '#7f3c8d', value: (s) => s.psiMemory, now: () => percent(census.value?.psi_memory) },
  { label: 'I/O', color: '#1f5fbf', value: (s) => s.psiIo, now: () => percent(census.value?.psi_io) },
];

const POOL: readonly Line[] = [
  {
    label: 'controller would set', color: '#0072B2', value: (s) => s.recommended, step: true,
    now: () => (census.value ? `${census.value.recommended_tokens} of ${store.tokens}` : ''),
  },
  { label: 'pool size', color: '#6e6e6e', value: (s) => s.tokens, step: true, dashed: true, now: () => String(store.tokens) },
];

// The compile cache: what share of the compiler calls of a minute it answered. A value in the legend is the newest
// minute's, then the last 24 hours' ("24 h:"); without a call there is no share, so it reads as a dash and not as 0 %.
const cachePoints = (): readonly CachePoint[] => cacheChart.value.points;
const newest = computed(() => (cacheChart.value.latest ? cachePoint(cacheChart.value.latest) : undefined));
const day = computed(() => (cacheDay.value ? cachePoint(cacheDay.value) : undefined));
const CACHE: readonly Line<CachePoint>[] = [
  { label: 'of all calls', color: '#0072B2', value: (p) => p.all, now: () => minuteAndDay(newest.value?.all, day.value?.all) },
  {
    label: 'of cacheable calls', color: '#8c6d31', value: (p) => p.cacheable, dashed: true,
    now: () => minuteAndDay(newest.value?.cacheable, day.value?.cacheable),
  },
];
const cacheEmpty = computed(() => emptyText(cacheChart.value.recorded, cacheChart.value.points.filter((p) => Number.isFinite(p.all)).length));
const cacheNote = computed(() => describeMinute(cacheChart.value.latest));
</script>

<template>
  <section class="trend" aria-label="Machine charts">
    <div>
      <SectionHead text="Cores: used on the whole machine" />
      <LinesChart :lines="CORES" :points="host" unit="cores" :min-top="store.cores" />
    </div>
    <div>
      <SectionHead text="Pressure: time some task waited (10 s)" />
      <LinesChart :lines="PRESSURE" :points="host" unit="%" :min-top="10" :max-top="100" />
    </div>
    <div>
      <SectionHead text="Pool size: slots, observed and not applied" />
      <LinesChart :lines="POOL" :points="host" unit="slots" :min-top="1" />
    </div>
    <div>
      <SectionHead text="Compile cache: calls it answered" />
      <LinesChart :lines="CACHE" :points="cachePoints" unit="%" :min-top="100" :max-top="100" :span="MINUTE_MS" :empty="cacheEmpty" :note="cacheNote" />
    </div>
  </section>
</template>

<style scoped>
/* Four charts in one row where each gets the 285 px of the widest heading; beside the output pane two rows of two; one column in a narrow window. */
.trend { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 0 24px; padding: 0 14px 4px; }
@container (max-width: 1239px) { .trend { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
@container (max-width: 639px) { .trend { grid-template-columns: minmax(0, 1fr); } }
.trend > div { min-width: 0; }
.trend :deep(h2) { margin-top: 12px; }
/*
 * The legend's height is kept, so the tables below do not move while the first reading has not arrived or when a number
 * gets one more digit: two lines where a chart is narrower than the 540 px of the widest legend (the compile cache's, its
 * minute and 24 hours at 100 %), one line where it is wider.
 */
.trend :deep(.legend) { min-height: 40px; align-content: flex-start; }
/* Four in a row: the plots give the second legend line its 20 px, so the tables start where they do beside one line. */
@container (min-width: 1240px) and (max-width: 2259px) { .trend { --plot-h: 60px; } }
@container (min-width: 1132px) and (max-width: 1239px) { .trend :deep(.legend) { min-height: 20px; } }
@container (min-width: 2260px) { .trend :deep(.legend) { min-height: 20px; } }
</style>
