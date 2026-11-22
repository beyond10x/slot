<script setup lang="ts">
import { computed } from 'vue';
import { fmtDur } from '../lib/format.ts';
import { sessionName } from '../lib/host.ts';
import { checkoutLabel, cmdText, jobId, waitS } from '../lib/job.ts';
import { level, levelText, servedQueue, waitReasonText } from '../lib/queue.ts';
import { jobs, select, sessionTitles, store } from '../store.ts';
import RepoName from './RepoName.vue';
import ToolDot from './ToolDot.vue';

const rows = computed(() => {
  const t = store.clock;
  return servedQueue(jobs.values()).map((j, k) => ({
    id: jobId(j),
    pos: k + 1,
    job: j,
    checkout: checkoutLabel(j, ' '),
    cmd: cmdText(j),
    session: sessionName(j.session, sessionTitles),
    wait: fmtDur(waitS(j, t)),
    level: j.ahead ? `${levelText(j)} · ahead` : levelText(j),
    levelTitle: j.ahead ? 'moved ahead of a job that came before it' : '',
    normal: level(j) === 'normal' && !j.ahead,
    slots: j.collected ? `${j.collected}/${j.tokens}` : String(j.tokens),
    slotsTitle: j.collected ? `holds ${j.collected} of the ${j.tokens} slots it asked for` : '',
    why: waitReasonText(j),
  }));
});
</script>

<template>
  <table class="jobs">
    <thead>
      <tr>
        <th class="c-lead num">pos</th><th class="c-job">job</th><th class="c-tool"><span>tool</span></th><th class="c-repo">repo</th>
        <th class="c-session h-b">session</th><th>command</th>
        <th class="c-state">level</th><th class="c-slots num">slots</th><th class="c-wait num">wait</th>
        <th class="c-run q-fill"></th><th class="c-cores q-fill"></th><th class="c-extra q-fill"></th><th class="c-mem q-fill"></th>
      </tr>
    </thead>
    <tbody>
      <template v-for="r in rows" :key="r.id">
        <tr :class="{ sel: store.selected === r.id, lead: !!r.why }" @click="select(r.id)">
          <td class="c-lead num">#{{ r.pos }}</td>
          <td><button type="button" class="jobid" :aria-label="`output of job ${r.id}`" @click.stop="select(r.id, $event)">{{ r.id }}</button></td>
          <td class="c-tool" :title="r.job.toolchain"><ToolDot :name="r.job.toolchain" /></td>
          <td class="repo" :title="`${r.checkout}\n${r.job.cwd}`"><RepoName :job="r.job" /></td>
          <td class="session h-b" :title="`${r.session}\nsession ${r.job.session}`">{{ r.session }}</td>
          <td class="cmd" :title="r.cmd">{{ r.cmd }}</td>
          <td class="c-state" :class="{ muted: r.normal }" :title="r.levelTitle || undefined">{{ r.level }}</td>
          <td class="num" :title="r.slotsTitle || undefined">{{ r.slots }}</td>
          <td class="num">{{ r.wait }}</td>
          <td class="q-fill" colspan="4"></td>
        </tr>
        <tr v-if="r.why" class="detail">
          <td></td><td colspan="7">{{ r.why }}</td><td class="h-b"></td><td class="q-fill" colspan="4"></td>
        </tr>
      </template>
      <tr v-if="!rows.length" class="empty"><td colspan="8">queue empty</td><td class="h-b"></td><td class="q-fill" colspan="4"></td></tr>
    </tbody>
  </table>
</template>

<style scoped>
/*
 * A queued job has no run time, cores or memory yet. Below 1800 px the Queue ends with wait and its command takes the room of
 * those columns: slots and wait then stand at the right edge, not under those of Running and Finished. From 1800 px the
 * command has room without it (612 px at a 1920 px window), and four empty columns keep slots and wait in line with the tables below.
 */
@container (max-width: 1799px) {
  .q-fill { display: none; }
}
/* A job with a reason line below it: the rule goes under the reason, so the two read as one row. */
/* No rule between a job and the line below it. A transparent rule would show the page through the hover colour. */
tr.lead td { border-bottom-width: 0; }
</style>
