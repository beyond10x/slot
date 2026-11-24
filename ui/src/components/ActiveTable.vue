<script setup lang="ts">
import { computed, reactive } from 'vue';
import { fmtBytes, fmtDur } from '../lib/format.ts';
import { sessionName } from '../lib/host.ts';
import { checkoutLabel, cmdText, jobId, runS, waitS , idLabel } from '../lib/job.ts';
import { shareCell } from '../lib/shares.ts';
import { phase } from '../lib/units.ts';
import { jobs, live, select, sessionTitles, sparks, store } from '../store.ts';
import CoresSpark from './CoresSpark.vue';
import RepoName from './RepoName.vue';
import ToolDot from './ToolDot.vue';
import UnitChips from './UnitChips.vue';

/** Jobs whose unit row the user opened. A row opens and closes only by its toggle, so no tick moves the rows below. */
const opened = reactive(new Set<number>());

const rows = computed(() => {
  const t = store.clock;
  const running = [...jobs.values()].filter((j) => j.state === 'running').sort((a, b) => jobId(a) - jobId(b));
  return running.map((j) => {
    const id = jobId(j);
    const l = live.get(id);
    const run = runS(j, t);
    return {
      id,
      job: j,
      checkout: checkoutLabel(j, ' '),
      session: sessionName(j.session, sessionTitles),
      live: l,
      open: opened.has(id),
      cmd: cmdText(j),
      phase: phase(l?.units ?? []),
      wait: fmtDur(waitS(j, t)),
      elapsed: run === null ? '' : fmtDur(run),
      cores: l ? l.cores.toFixed(1) : '',
      rss: l ? fmtBytes(l.rss) : '',
      spark: [...sparks.get(id)],
      slots: shareCell(j),
    };
  });
});

function toggle(id: number, open: boolean): void {
  if (open) opened.delete(id);
  else opened.add(id);
}
</script>

<template>
  <table class="jobs">
    <thead>
      <tr>
        <th class="c-lead"><span class="sr">units</span></th><th class="c-job">job</th><th class="c-tool"><span>tool</span></th><th class="c-repo">repo</th>
        <th class="c-session h-b">session</th><th>command</th><th class="c-out h-c">output</th>
        <th class="c-state">phase</th><th class="c-slots num">{{ store.shares ? 'share' : 'slots' }}</th><th class="c-wait num">wait</th><th class="c-run num">elapsed</th>
        <th class="c-cores num">cores</th><th class="c-extra h-a">last 60 s</th><th class="c-mem num h-a">RSS</th>
      </tr>
    </thead>
    <tbody>
      <template v-for="r in rows" :key="r.id">
        <tr :class="{ sel: store.selected === r.id, lead: r.open }" @click="select(r.id)">
          <td class="c-lead">
            <button type="button" class="tg" :aria-expanded="r.open" :title="r.open ? 'hide units' : 'show units'"
              :aria-label="`${r.open ? 'hide' : 'show'} units of job ${r.id}`" @click.stop="toggle(r.id, r.open)">
              <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true" :class="{ open: r.open }"><path d="M3 1.5 7 5 3 8.5" /></svg>
            </button>
          </td>
          <td><button type="button" class="jobid" :aria-label="`output of job ${r.id}`" @click.stop="select(r.id, $event)">{{ idLabel(r.job) }}</button></td>
          <td class="c-tool" :title="r.job.toolchain"><ToolDot :name="r.job.toolchain" /></td>
          <td class="repo" :title="`${r.checkout}\n${r.job.cwd}`"><RepoName :job="r.job" /></td>
          <td class="session h-b" :title="`${r.session}\nsession ${r.job.session}`">{{ r.session }}</td>
          <td class="cmd" :title="r.cmd">{{ r.cmd }}</td>
          <td class="c-out h-c">
            <button type="button" class="btn live" :class="{ on: store.selected === r.id }" tabindex="-1" title="watch this job's output as it is written"
              @click.stop="select(r.id, $event)"><i></i>live</button>
          </td>
          <td class="c-state"><span class="phase" :class="r.phase">{{ r.phase }}</span></td>
          <td class="num" :title="r.slots.title || undefined">{{ r.slots.text }}</td>
          <td class="num">{{ r.wait }}</td>
          <td class="num">{{ r.elapsed }}</td>
          <td class="num">{{ r.cores }}</td>
          <td class="c-extra h-a"><CoresSpark :points="r.spark" :t1="store.clock" :span="sparks.spanMs" /></td>
          <td class="num h-a">{{ r.rss }}</td>
        </tr>
        <tr v-if="r.open" class="detail">
          <td></td><td colspan="9"><UnitChips :live="r.live" /></td><td class="h-b"></td><td class="h-c"></td><td class="h-a"></td><td class="h-a"></td>
        </tr>
      </template>
      <tr v-if="!rows.length" class="empty"><td colspan="10">nothing running</td><td class="h-b"></td><td class="h-c"></td><td class="h-a"></td><td class="h-a"></td></tr>
    </tbody>
  </table>
</template>

<style scoped>
.tg { display: inline-flex; align-items: center; justify-content: center; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 4px;
  background: none; color: var(--ink); cursor: pointer; }
.tg:hover { background: #e3e6ea; }
.tg svg { fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
.tg svg.open { transform: rotate(90deg); }
/* "live" marks a stream that carries output: the mark is the calm ink, red stays the colour of a failure. */
.live { padding: 0 7px 0 6px; }
.live i { display: inline-block; width: 7px; height: 7px; margin-right: 5px; border-radius: 50%; background: #1e8e3e; }
.live.on i { background: #8fe0a6; }
/* No rule between a job and the line below it. A transparent rule would show the page through the hover colour. */
tr.lead td { border-bottom-width: 0; }
.phase { display: inline-block; padding: 0 7px; border-radius: 10px; background: #ececec; font-size: 13px; line-height: 20px; }
.phase.compiling { background: #dbeaf5; }
.phase.testing { background: #d6efe6; }
.phase.linking { background: #e4e4e4; }
</style>
