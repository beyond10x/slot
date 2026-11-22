<script setup lang="ts">
import { census, store } from '../store.ts';
import LinesChart from './LinesChart.vue';
import type { Line } from './LinesChart.vue';
import SectionHead from './SectionHead.vue';

// The three line charts of the machine, above the job tables: what the host does comes before who waits for it.
const percent = (v: number | undefined): string => (v === undefined ? '' : `${v.toFixed(0)} %`);
const cores = (v: number | undefined): string => (v === undefined ? '' : v.toFixed(1));

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
</script>

<template>
  <section class="trend" aria-label="Machine charts">
    <div>
      <SectionHead text="Cores: used on the whole machine" />
      <LinesChart :lines="CORES" unit="cores" :min-top="store.cores" />
    </div>
    <div>
      <SectionHead text="Pressure: time some task waited (10 s average)" />
      <LinesChart :lines="PRESSURE" unit="%" :min-top="10" :max-top="100" />
    </div>
    <div>
      <SectionHead text="Pool size: slots, observed and not applied" />
      <LinesChart :lines="POOL" unit="slots" :min-top="1" />
    </div>
  </section>
</template>

<style scoped>
.trend { display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 0 24px; padding: 0 14px 4px; }
.trend > div { min-width: 0; }
.trend :deep(h2) { margin-top: 12px; }
/* One legend line is kept even while the first reading has not arrived, so the tables below do not move. */
.trend :deep(.legend) { min-height: 20px; }
</style>
