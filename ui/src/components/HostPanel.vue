<script setup lang="ts">
import { familyColor } from '../lib/host.ts';
import { store } from '../store.ts';
import HostChart from './HostChart.vue';
import LinesChart from './LinesChart.vue';
import type { Line } from './LinesChart.vue';
import SessionsTable from './SessionsTable.vue';

const PSI: readonly Line[] = [
  { label: 'cpu', color: '#c4302b', value: (s) => s.psiCpu },
  { label: 'memory', color: '#7F3C8D', value: (s) => s.psiMemory },
  { label: 'io', color: '#1f5fbf', value: (s) => s.psiIo },
];

const POOL: readonly Line[] = [
  { label: 'recommended (observe, not enforced)', color: '#0072B2', value: (s) => s.recommended, step: true },
  { label: 'pool size', color: '#888888', value: (s) => s.tokens, step: true, dashed: true },
];
</script>

<template>
  <section>
    <h2>Host: build cores by tool family on the whole machine. Solid: run by slot. Hatched: not on slot.</h2>
    <HostChart />
    <div class="legend">
      <span v-for="f in store.families" :key="f">
        <i :style="{ background: familyColor(f) }"></i><i class="hatched" :style="{ background: familyColor(f) }"></i>{{ f }}
      </span>
      <span v-if="!store.families.length" class="muted">no build processes seen yet</span>
    </div>
    <div class="small">
      <div>
        <h2>Pressure stall, some avg10 (%)</h2>
        <LinesChart :lines="PSI" unit="%" :min-top="10" :max-top="100" />
      </div>
      <div>
        <h2>Pool size: controller recommendation</h2>
        <LinesChart :lines="POOL" unit="slots" :min-top="1" />
      </div>
    </div>
    <h2>Sessions: cores per agent session now, highlighted when its builds bypass slot</h2>
    <SessionsTable />
  </section>
</template>

<style scoped>
section { padding: 8px 14px 0; }
.legend { display: flex; flex-wrap: wrap; gap: 2px 12px; font-size: 12px; margin-top: 3px; }
.legend i { display: inline-block; width: 9px; height: 9px; vertical-align: 0; }
.legend i.hatched { margin: 0 5px 0 1px; opacity: 0.5;
  background-image: repeating-linear-gradient(135deg, rgba(255, 255, 255, 0.9) 0 1.5px, transparent 1.5px 4px) !important; }
.small { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 0 18px; }
</style>
