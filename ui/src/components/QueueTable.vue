<script setup lang="ts">
import { computed } from 'vue';
import { fmtDur } from '../lib/format.ts';
import { cmdText, jobId, waitS } from '../lib/job.ts';
import { level, levelText, servedQueue } from '../lib/queue.ts';
import { jobs, select, store } from '../store.ts';
import ToolDot from './ToolDot.vue';

const rows = computed(() => {
  const t = store.clock;
  return servedQueue(jobs.values()).map((j, k) => ({
    id: jobId(j),
    pos: k + 1,
    job: j,
    cmd: cmdText(j),
    wait: fmtDur(waitS(j, t)),
    level: levelText(j),
    normal: level(j) === 'normal',
  }));
});
</script>

<template>
  <table>
    <thead>
      <tr>
        <th class="num">pos</th><th>#</th><th>tool</th><th>command</th><th>repo</th><th class="opt">session</th>
        <th>level</th><th class="num">slots</th><th class="num">wait</th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="r in rows" :key="r.id" :class="{ sel: store.selected === r.id }" @click="select(r.id)">
        <td class="num">#{{ r.pos }}</td>
        <td><button type="button" class="jobid" :aria-label="`output of job ${r.id}`" @click.stop="select(r.id)">{{ r.id }}</button></td>
        <td><ToolDot :name="r.job.toolchain" /></td>
        <td class="cmd" :title="r.cmd">{{ r.cmd }}</td>
        <td class="repo" :title="r.job.repo">{{ r.job.repo }}</td>
        <td class="muted opt">{{ r.job.session.slice(0, 8) }}</td>
        <td :class="{ muted: r.normal }">{{ r.level }}</td>
        <td class="num">{{ r.job.tokens }}</td>
        <td class="num" :title="r.job.wait_reason || undefined">{{ r.wait }}<span v-if="r.job.wait_reason" class="why"> · {{ r.job.wait_reason.includes('filesystem') ? 'disk' : 'memory' }}</span></td>
      </tr>
      <tr v-if="!rows.length" class="empty"><td colspan="9" class="muted">queue empty</td></tr>
    </tbody>
  </table>
</template>
