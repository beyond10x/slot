<script setup lang="ts">
import { computed, watch, watchEffect } from 'vue';
import { fmtDur } from './lib/format.ts';
import { jobId, newestRunning } from './lib/job.ts';
import { queueHeading, servedQueue, waitStats } from './lib/queue.ts';
import { jobs, select, store } from './store.ts';
import HeaderBar from './components/HeaderBar.vue';
import HostPanel from './components/HostPanel.vue';
import TimelineChart from './components/TimelineChart.vue';
import CoresChart from './components/CoresChart.vue';
import QueueTable from './components/QueueTable.vue';
import ActiveTable from './components/ActiveTable.vue';
import DoneTable from './components/DoneTable.vue';
import OutputPane from './components/OutputPane.vue';
import SectionHead from './components/SectionHead.vue';
import ToolDot from './components/ToolDot.vue';
import UsageTable from './components/UsageTable.vue';

const newest = computed(() => {
  const j = newestRunning(jobs.values());
  return j ? jobId(j) : null;
});

const queue = computed(() => servedQueue(jobs.values()));
const queueTitle = computed(() => queueHeading(queue.value));
/** Slot wait of the jobs that finished in the last 60 minutes, in words. */
const waits = computed(() => {
  const w = waitStats(jobs.values(), store.clock);
  const dur = (v: number | null): string => (v === null ? '–' : fmtDur(v));
  return { n: w.n, median: dur(w.p50), p90: dur(w.p90), gaveUp: w.gaveUp };
});
const running = computed(() => {
  let n = 0;
  for (const j of jobs.values()) if (j.state === 'running') n++;
  return { n };
});

watch(
  [newest, () => store.followNewest],
  ([id, on]) => {
    if (on && id !== null && id !== store.selected) select(id);
  },
  { immediate: true },
);

// The tab names what a glance at it should tell: how many jobs run and wait, then the host.
watchEffect(() => {
  if (!store.host) return;
  document.title = `${running.value.n} running, ${queue.value.length} queued · slot ${store.host}`;
});
</script>

<template>
  <HeaderBar />
  <p v-if="!store.ready" class="muted waiting">loading jobs and history from the daemon…</p>
  <main v-else :class="{ nopane: store.selected === null }">
    <div>
      <section class="tables">
        <SectionHead :text="queueTitle" />
        <p v-if="waits.n" class="sum">
          slot wait of <b>{{ waits.n }}</b> {{ waits.n === 1 ? 'job' : 'jobs' }} finished in the last 60 min: median <b>{{ waits.median }}</b>, 9 of 10 within
          <b>{{ waits.p90 }}</b>, <b>{{ waits.gaveUp }}</b> cancelled while queued
        </p>
        <p v-else class="sum">no job finished in the last 60 min</p>
        <QueueTable />
        <SectionHead text="Running" />
        <p class="sum">
          <b>{{ running.n }}</b> {{ running.n === 1 ? 'job uses' : 'jobs use' }} <b>{{ store.jobCores.toFixed(1) }}</b> cores
        </p>
        <ActiveTable />
        <SectionHead text="Finished" />
        <DoneTable />
      </section>
      <HostPanel />
      <section>
        <SectionHead text="Timeline: one bar per job. Pale part: waiting for a slot. Click a bar for its output." />
        <TimelineChart />
        <SectionHead text="Cores in use: slot jobs (by toolchain) and whole host; below: jobs waiting for a slot" />
        <CoresChart />
        <div class="legend">
          <ToolDot v-for="n in store.toolchains" :key="n" :name="n" />
          <span>slot jobs <b>{{ store.jobCores.toFixed(1) }}</b> cores</span>
          <span><i class="hostline"></i>whole host <b>{{ (store.sysCpu * 100).toFixed(0) }} %</b></span>
        </div>
      </section>
      <section>
        <SectionHead text="Share of the pool: slot time (run time × slots held) and CPU per project, repository or session" />
        <UsageTable />
      </section>
    </div>
    <OutputPane v-if="store.selected !== null" :id="store.selected" :key="store.selected" />
  </main>
</template>

<style scoped>
main { display: grid; grid-template-columns: minmax(0, 1fr) minmax(320px, 38%); }
main.nopane { grid-template-columns: minmax(0, 1fr); }
main > div { container-type: inline-size; min-width: 0; padding-bottom: 24px; }
/* Below 1100 px the output pane is a sheet along the bottom (OutputPane.vue); the tables keep the full width. */
@media (max-width: 1099px) {
  main { grid-template-columns: minmax(0, 1fr); }
  main:not(.nopane) { padding-bottom: 45vh; }
}
section { padding: 2px 14px 6px; }
/* A heading and the numbers of its table share a line where both fit; the table takes the full width below them. */
.tables { display: flex; flex-wrap: wrap; align-items: baseline; column-gap: 20px; }
.tables > :deep(h2) { flex: 0 1 auto; }
.tables > .sum { flex: 0 1 auto; margin: 0 0 4px auto; }
.tables > :deep(table) { flex: 0 0 calc(100% + 12px); min-width: 0; }
.tables > :deep(h2):nth-of-type(3) { flex-basis: 100%; }
.hostline { display: inline-block; width: 14px; height: 0; border-top: 2px solid #222; vertical-align: middle; margin-right: 5px; }
.waiting { margin: 0; padding: 16px 14px; }
</style>
