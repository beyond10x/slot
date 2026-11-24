<script setup lang="ts">
import { computed } from 'vue';
import { fmtBytes, fmtClock, fmtDur } from '../lib/format.ts';
import { sessionName } from '../lib/host.ts';
import { checkoutLabel, cmdText, idLabel, jobKey, jobName, resultText, runS, splitJobs, times, waitS } from '../lib/job.ts';
import { jobs, select, sessionTitles, store } from '../store.ts';
import RepoName from './RepoName.vue';
import ToolDot from './ToolDot.vue';

const rows = computed(() =>
  splitJobs(jobs.values()).done.map((j) => {
    const end = times(j).ended ?? 0;
    const dur = runS(j, end) ?? 0;
    return {
      id: jobKey(j),
      job: j,
      checkout: checkoutLabel(j, ' '),
      session: sessionName(j.session, sessionTitles),
      cmd: cmdText(j),
      result: resultText(j),
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
  <table class="jobs">
    <thead>
      <tr>
        <th class="c-lead"></th><th class="c-job">job</th><th class="c-tool"><span>tool</span></th><th class="c-repo">repo</th>
        <th class="c-session h-b">session</th><th>command</th><th class="c-out h-c">ended</th>
        <th class="c-state">result</th><th class="c-slots num">slots</th><th class="c-wait num">wait</th><th class="c-run num">duration</th>
        <th class="c-cores num">avg cores</th><th class="c-extra num h-a">CPU s</th><th class="c-mem num h-a">peak RSS</th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="r in rows" :key="r.id" :class="{ sel: store.selected === r.id }" @click="select(r.id)">
        <td class="c-lead mark" :class="r.job.state"></td>
        <td><button type="button" class="jobid" :aria-label="`output of job ${jobName(r.job)}`" @click.stop="select(r.id, $event)">{{ idLabel(r.job) }}</button></td>
        <td class="c-tool" :title="r.job.toolchain"><ToolDot :name="r.job.toolchain" /></td>
        <td class="repo" :title="`${r.checkout}\n${r.job.cwd}`"><RepoName :job="r.job" /></td>
        <td class="session h-b" :title="`${r.session}\nsession ${r.job.session}`">{{ r.session }}</td>
        <td class="cmd" :title="r.cmd">{{ r.cmd }}</td>
        <td class="c-out h-c">{{ r.ended }}</td>
        <td class="c-state" :class="{ bad: r.bad }">{{ r.result }}</td>
        <td class="num">{{ r.job.tokens }}</td>
        <td class="num">{{ r.wait }}</td>
        <td class="num">{{ r.dur }}</td>
        <td class="num">{{ r.avg }}</td>
        <td class="num h-a">{{ r.cpu }}</td>
        <td class="num h-a">{{ r.rss }}</td>
      </tr>
      <tr v-if="!rows.length" class="empty"><td colspan="10">nothing finished yet</td><td class="h-b"></td><td class="h-c"></td><td class="h-a"></td><td class="h-a"></td></tr>
    </tbody>
  </table>
</template>

<style scoped>
/* The mark repeats the result column for the eye, so it is drawn, not text: a reader of the text gets the word once. */
.mark { text-align: center; padding-left: 0; }
.mark.ok::before { content: '✓'; }
.mark.failed::before { content: '✕'; }
.mark.cancelled::before { content: '–'; }
.mark.failed { color: var(--bad); font-weight: 700; }
</style>
