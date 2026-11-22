<script setup lang="ts">
import { computed } from 'vue';
import { machineDetail } from '../lib/headroom.ts';
import { familyColor } from '../lib/host.ts';
import { census, store } from '../store.ts';
import HostChart from './HostChart.vue';
import SectionHead from './SectionHead.vue';
import SessionsTable from './SessionsTable.vue';

const machine = computed(() => machineDetail(store.machine, store.cores, store.cpuQuota));
</script>

<template>
  <section>
    <div class="head">
      <SectionHead text="Host: build cores by tool family on the whole machine. Solid: run by slot. Hatched: not on slot." />
      <p v-if="census" class="sum">
        all programs use <b>{{ census.host_cores.toFixed(1) }}</b> of {{ store.cores }} cores, builds <b>{{ census.build_cores.toFixed(1) }}</b>,
        of them not on slot <b>{{ census.unmanaged_cores.toFixed(1) }}</b>
      </p>
    </div>
    <HostChart />
    <div class="legend">
      <span v-for="f in store.families" :key="f">
        <i :style="{ background: familyColor(f) }"></i><i class="hatched" :style="{ background: familyColor(f) }"></i>{{ f }}
      </span>
      <span v-if="!store.families.length">no build processes seen yet</span>
    </div>
    <p v-if="machine" class="machine">{{ machine }}</p>
    <SectionHead text="Sessions: cores per agent session now, highlighted when its builds not on slot use more than 1 core" />
    <SessionsTable />
  </section>
</template>

<style scoped>
section { padding: 2px 14px 6px; }
.head { display: flex; flex-wrap: wrap; align-items: baseline; column-gap: 20px; }
.head .sum { margin: 0 0 4px auto; }
.legend i { display: inline-block; width: 9px; height: 9px; border-radius: 2px; }
.legend i.hatched { margin: 0 6px 0 1px; opacity: 0.5;
  background-image: repeating-linear-gradient(135deg, rgba(255, 255, 255, 0.9) 0 1.5px, transparent 1.5px 4px) !important; }
.machine { max-width: 110ch; margin: 8px 0 0; font-size: 13px; color: var(--ink); font-variant-numeric: tabular-nums; }
</style>
