<script setup lang="ts">
import { computed, watch } from 'vue';
import { jobId, newestRunning } from './lib/job.ts';
import { jobs, select, store } from './store.ts';
import HeaderBar from './components/HeaderBar.vue';
import HostPanel from './components/HostPanel.vue';
import TimelineChart from './components/TimelineChart.vue';
import CoresChart from './components/CoresChart.vue';
import QueueTable from './components/QueueTable.vue';
import ActiveTable from './components/ActiveTable.vue';
import DoneTable from './components/DoneTable.vue';
import OutputPane from './components/OutputPane.vue';
import UsageTable from './components/UsageTable.vue';

const newest = computed(() => {
  const j = newestRunning(jobs.values());
  return j ? jobId(j) : null;
});

watch(
  [newest, () => store.followNewest],
  ([id, on]) => {
    if (on && id !== null && id !== store.selected) select(id);
  },
  { immediate: true },
);
</script>

<template>
  <HeaderBar />
  <p v-if="!store.ready" class="muted waiting">loading jobs and history from the daemon…</p>
  <main v-else :class="{ nopane: store.selected === null }">
    <div>
      <HostPanel />
      <section>
        <h2>Timeline: one bar per job. Pale part: waiting for a slot. Click a bar for its output.</h2>
        <TimelineChart />
        <h2>Cores in use: slot jobs (by toolchain) and whole host; below: jobs waiting for a slot</h2>
        <CoresChart />
      </section>
      <section>
        <h2>Queue: waiting for slots, served oldest first</h2>
        <QueueTable />
        <h2>Running</h2>
        <ActiveTable />
        <h2>Finished</h2>
        <DoneTable />
      </section>
      <section>
        <h2>Share of the pool: slot time (run time × slots held) and CPU per project, repository or session</h2>
        <UsageTable />
      </section>
    </div>
    <OutputPane v-if="store.selected !== null" :id="store.selected" :key="store.selected" />
  </main>
</template>

<style scoped>
main { display: grid; grid-template-columns: minmax(0, 1fr) minmax(320px, 38%); }
main.nopane { grid-template-columns: minmax(0, 1fr); }
main > div { container-type: inline-size; min-width: 0; }
/* Below 1100 px the output pane is a sheet along the bottom (OutputPane.vue); the tables keep the full width. */
@media (max-width: 1099px) {
  main { grid-template-columns: minmax(0, 1fr); }
  main:not(.nopane) { padding-bottom: 45vh; }
}
section { padding: 8px 14px; }
.waiting { padding: 8px 14px; }
</style>
