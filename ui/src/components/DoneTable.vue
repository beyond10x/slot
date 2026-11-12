<script setup lang="ts">
import { computed } from 'vue';
import { fmtBytes, fmtClock, fmtDur } from '../lib/format.ts';
import { cmdText, jobId, runS, splitJobs, times, waitS } from '../lib/job.ts';
import { jobs, select, store } from '../store.ts';
import ToolDot from './ToolDot.vue';

const rows = computed(() =>
  splitJobs(jobs.values()).done.map((j) => {
    const end = times(j).ended ?? 0;
    const dur = runS(j, end) ?? 0;
    return {
      id: jobId(j),
      job: j,
      cmd: cmdText(j),
      exit: j.state === 'cancelled' ? 'cancelled' : String(j.exit),
      bad: j.exit !== 0,
      wait: fmtDur(waitS(j, end)),
      dur: fmtDur(dur),
      cpu: j.cpu_s ? j.cpu_s.toFixed(0) : '',
      avg: dur > 0 && j.cpu_s ? (j.cpu_s / dur).toFixed(1) : '',
      rss: fmtBytes(Number(j.peak_rss)),
      ended: end ? fmtClock(end) : '',
    };
  }),
);
</script>

<template>
  <table>
    <thead>
      <tr>
        <th>#</th><th>tool</th><th>repo</th><th>command</th><th>exit</th>
        <th class="num">wait</th><th class="num">duration</th><th class="num">CPU s</th><th class="num opt">avg cores</th>
        <th class="num opt">peak RSS</th><th>ended</th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="r in rows" :key="r.id" :class="{ sel: store.selected === r.id }" @click="select(r.id)">
        <td><button type="button" class="jobid" :aria-label="`output of job ${r.id}`" @click.stop="select(r.id)">{{ r.id }}</button></td>
        <td><ToolDot :name="r.job.toolchain" /></td>
        <td class="repo" :title="r.job.repo">{{ r.job.repo }}</td>
        <td class="cmd" :title="r.cmd">{{ r.cmd }}</td>
        <td :class="{ bad: r.bad }">{{ r.exit }}</td>
        <td class="num">{{ r.wait }}</td>
        <td class="num">{{ r.dur }}</td>
        <td class="num">{{ r.cpu }}</td>
        <td class="num opt">{{ r.avg }}</td>
        <td class="num opt">{{ r.rss }}</td>
        <td class="muted">{{ r.ended }}</td>
      </tr>
    </tbody>
  </table>
</template>
